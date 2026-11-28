#![deny(clippy::unwrap_used, clippy::expect_used)]

mod archive;
mod files;
mod notes;
mod report;
mod rpc;
mod spaces;

use clap::Parser;
use report::Report;
use rpc::{Client, Result};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

const ORIGIN: &str = "pensine-old";

#[derive(Parser)]
#[command(about = "Copy an old Pensine node to a new Pensine server over MCP")]
struct Args {
    #[arg(long)]
    from: String,
    #[arg(long)]
    from_key_env: String,
    #[arg(long)]
    to: String,
    #[arg(long)]
    to_key_env: String,
    #[arg(long = "map", value_parser = parse_mapping)]
    mappings: Vec<(String, String)>,
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
    #[arg(long)]
    include_retired: bool,
    #[arg(long)]
    archive: Option<PathBuf>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long, default_value = "100", value_parser = clap::value_parser!(u16).range(1..))]
    batch: u16,
    /// Files larger than this many megabytes are left on the source.
    #[arg(long, default_value = "40", value_parser = clap::value_parser!(u64).range(1..=60))]
    max_file_mb: u64,
}

fn parse_mapping(value: &str) -> Result<(String, String)> {
    let (old, new) = value.split_once('=').ok_or("Expected OLD=NEW")?;
    if old.is_empty() || !spaces::valid_target(new) {
        return Err("Expected an old space and a valid new space name".into());
    }
    Ok((old.into(), new.into()))
}

struct Migration<'a> {
    args: &'a Args,
    old: Client<'a>,
    new: Option<Client<'a>>,
}

impl Migration<'_> {
    fn run(&self, output: &mut String) -> Result<bool> {
        let inventory = self.old.call("spaces", json!({}))?.data;
        let plan = spaces::plan(&inventory, &self.args.only, &self.args.mappings)?;
        let mut existing = if let Some(new) = &self.new {
            let spaces = new.call("spaces", json!({}))?.data;
            rpc::array(&spaces)?
                .iter()
                .map(|entry| rpc::text(&entry["name"]).map(str::to_owned))
                .collect::<Result<BTreeSet<_>>>()?
        } else {
            BTreeSet::new()
        };
        if self.args.dry_run {
            output.push_str("Dry run: source inventory only\n");
        }
        let mut reports = Vec::new();
        let mut targets = BTreeSet::new();
        if let Some(new) = &self.new {
            for space in &plan {
                spaces::create(new, space, &mut existing)?;
            }
        }
        let todos = self.todo_ids()?;
        let stats = self
            .args
            .include_retired
            .then(|| {
                self.old
                    .call("stats", json!({}))
                    .ok()
                    .map(|reply| reply.data)
            })
            .flatten();
        for space in plan.iter().filter(|space| space.selected) {
            reports.push(self.copy_space(
                space,
                &mut existing,
                &mut targets,
                stats.as_ref(),
                &todos,
            )?);
        }
        Ok(self.verify(output, reports, targets))
    }

    fn copy_space(
        &self,
        space: &spaces::Space,
        existing: &mut BTreeSet<String>,
        targets: &mut BTreeSet<String>,
        stats: Option<&Value>,
        todos: &BTreeMap<String, String>,
    ) -> Result<Report> {
        let mut report = Report::new(space.old.clone());
        targets.insert(space.new.clone());
        report.add_target(&space.new);
        if space.routes_to_inbox() && existing.contains("inbox") {
            targets.insert("inbox".into());
            report.add_target("inbox");
        }
        let mut archive = self
            .args
            .archive
            .as_deref()
            .map(|root| archive::Archive::new(root, &space.old))
            .transpose()?;
        if let Some(archive) = &mut archive {
            archive.entry("space", &space.raw)?;
        }
        let notes = notes::read(
            &self.old,
            &space.old,
            self.args.include_retired,
            notes::retired_count(stats, &space.old),
            &mut report,
        );
        for note in notes {
            if let Some(archive) = &mut archive {
                archive.entry("note", &note.raw)?;
            }
            let target = space.note_target(&note);
            if let Some(new) = &self.new {
                if let Err(error) = spaces::create_inbox(new, target, existing) {
                    report.error(format!("inbox creation: {error}"));
                }
            }
            targets.insert(target.to_owned());
            report.add_note(target, note);
        }
        if let Some(new) = &self.new {
            report.send_notes(new, usize::from(self.args.batch), todos);
        }
        files::copy(
            &self.old,
            self.new.as_ref(),
            space,
            archive.as_mut(),
            &mut report,
            self.args.max_file_mb * 1_000_000,
        )?;
        if let Some(archive) = archive {
            archive.finish()?;
        }
        Ok(report)
    }

    fn todo_ids(&self) -> Result<BTreeMap<String, String>> {
        let Some(new) = &self.new else {
            return Ok(BTreeMap::new());
        };
        let spaces = new.call("spaces", json!({}))?.data;
        let mut todos = BTreeMap::new();
        for space in rpc::array(&spaces)? {
            if let Some(todo) = space["todo"].as_str() {
                todos.insert(rpc::text(&space["name"])?.to_owned(), todo.to_owned());
            }
        }
        Ok(todos)
    }

    fn verify(&self, output: &mut String, reports: Vec<Report>, targets: BTreeSet<String>) -> bool {
        let readback = self.new.as_ref().map(|new| {
            targets
                .into_iter()
                .map(|target| {
                    let pairs = notes::read_back(new, &target, self.args.include_retired);
                    (target, pairs)
                })
                .collect::<BTreeMap<_, _>>()
        });
        let mut good = true;
        let mut matched = BTreeSet::new();
        for mut report in reports {
            good &= report.finish(output, readback.as_ref(), &mut matched);
        }
        if let Some(readback) = readback.filter(|_| self.args.only.is_empty()) {
            for (target, pairs) in readback {
                if let Ok(pairs) = pairs {
                    for id in pairs
                        .keys()
                        .filter(|id| !matched.contains(&(target.clone(), (*id).clone())))
                    {
                        good = false;
                        report::line(
                            output,
                            format_args!("integrity {target:?}: unexpected origin {id:?}"),
                        );
                    }
                }
            }
        }
        good
    }
}

fn read_key(name: &str) -> Result<String> {
    env::var(name)
        .ok()
        .filter(|key| !key.is_empty() && key.bytes().all(|c| c.is_ascii_graphic()))
        .ok_or_else(|| "A key environment variable is missing, empty or invalid".into())
}

fn execute(args: &Args, output: &mut String) -> Result<(bool, Vec<String>)> {
    let old_key = read_key(&args.from_key_env)?;
    let new_key = (!args.dry_run)
        .then(|| read_key(&args.to_key_env))
        .transpose()?;
    let result = (|| {
        let old = Client::new(&args.from, &old_key)?;
        let new = new_key
            .as_deref()
            .map(|key| Client::new(&args.to, key))
            .transpose()?;
        Migration { args, old, new }.run(output)
    })();
    let ok = match result {
        Ok(ok) => ok,
        Err(error) => {
            report::line(output, format_args!("Migration failed: {error}"));
            false
        }
    };
    Ok((ok, std::iter::once(old_key).chain(new_key).collect()))
}

fn main() -> ExitCode {
    let args = Args::parse();
    let mut output = String::new();
    match execute(&args, &mut output) {
        Ok((ok, mut keys)) => {
            keys.sort_by_key(|key| std::cmp::Reverse(key.len()));
            for key in keys {
                output = output.replace(&key, "[redacted]");
            }
            if io::stdout().lock().write_all(output.as_bytes()).is_ok() && ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
