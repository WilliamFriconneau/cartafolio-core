use crate::Result;
use clap::{Args, Subcommand};
use pensine_access::{AccessError, Accounts, Grant};
use std::io::Write;

#[derive(Subcommand)]
pub(crate) enum AccountCommand {
    /// Create an account and its store.
    Create {
        id: String,
        /// Public URL alias for the account.
        #[arg(long, value_name = "NAME")]
        alias: Option<String>,
    },
    /// List account ids and aliases.
    List,
    /// Change the public alias of an account (the last part of its MCP address).
    Alias {
        #[arg(long, value_name = "ID")]
        account: String,
        /// New alias: 2 to 32 characters of a-z, 0-9 and '-'.
        name: String,
    },
    /// Link a Home Assistant user to an existing account.
    Link {
        #[arg(long, value_name = "ID")]
        account: String,
        /// The Home Assistant user id (shown in the user's profile).
        #[arg(long = "ha-user", value_name = "ID")]
        ha_user: String,
        /// Display name kept with the link.
        #[arg(long, value_name = "NAME", default_value = "")]
        name: String,
    },
}

impl AccountCommand {
    pub(crate) fn run(self, accounts: &Accounts, out: &mut impl Write) -> Result<()> {
        match self {
            Self::Create { id, alias } => {
                accounts.account_create_with_alias(&id, alias.as_deref())?;
                writeln!(out, "Created account {id}")?;
            }
            Self::List => {
                for (id, alias) in accounts.account_list()? {
                    writeln!(out, "{id}\t{alias}")?;
                }
            }
            Self::Alias { account, name } => {
                accounts.set_alias(&account, &name)?;
                writeln!(out, "Alias of account {account} is now {name}")?;
            }
            Self::Link {
                account,
                ha_user,
                name,
            } => {
                accounts.link_ha_user(&ha_user, &account, &name)?;
                writeln!(out, "Linked Home Assistant user to account {account}")?;
            }
        }
        Ok(())
    }
}

#[derive(Subcommand)]
pub(crate) enum KeyCommand {
    /// Create a bearer key and print it once.
    Create(KeyCreate),
    /// List connections without their keys.
    List {
        #[arg(long, value_name = "ID")]
        account: String,
    },
    /// Revoke the connection with the given name.
    Revoke {
        #[arg(long, value_name = "ID")]
        account: String,
        #[arg(long)]
        name: String,
    },
}

#[derive(Args)]
pub(crate) struct KeyCreate {
    #[arg(long, value_name = "ID")]
    account: String,
    #[arg(long)]
    name: String,
    /// Disable writes even when a grant allows them.
    #[arg(long)]
    read_only: bool,
    /// Grant read access to SPACE, or write access with SPACE:rw.
    #[arg(long = "grant", value_name = "SPACE[:rw]", value_parser = parse_grant)]
    grants: Vec<Grant>,
    /// Grant access to all spaces, including future spaces.
    #[arg(long)]
    all: bool,
}

impl KeyCommand {
    pub(crate) fn run(self, accounts: &Accounts, out: &mut impl Write) -> Result<()> {
        match self {
            Self::Create(args) => args.run(accounts, out),
            Self::List { account } => {
                for connection in accounts.connections(&account)? {
                    let grants = connection
                        .grants
                        .iter()
                        .map(|grant| {
                            if grant.write && !connection.read_only {
                                format!("{}:rw", grant.space)
                            } else {
                                grant.space.clone()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    let mode = if connection.read_only {
                        "read-only"
                    } else {
                        "scoped"
                    };
                    writeln!(
                        out,
                        "{}\t{}\t{mode}\t{grants}",
                        connection.id,
                        connection.name.escape_debug()
                    )?;
                }
                Ok(())
            }
            Self::Revoke { account, name } => revoke(accounts, &account, &name, out),
        }
    }
}

impl KeyCreate {
    fn run(mut self, accounts: &Accounts, out: &mut impl Write) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err("Connection name must not be empty".into());
        }
        if accounts
            .connections(&self.account)?
            .iter()
            .any(|connection| connection.name == self.name)
        {
            return Err("A connection with that name already exists".into());
        }
        if self.all {
            self.grants.push(Grant {
                space: "*".into(),
                write: true,
            });
        }
        let (_, key) =
            accounts.connection_create(&self.account, &self.name, self.grants, self.read_only)?;
        writeln!(out, "{}", key.as_str())?;
        Ok(())
    }
}

fn parse_grant(value: &str) -> std::result::Result<Grant, String> {
    let (space, write) = match value.strip_suffix(":rw") {
        Some(space) => (space, true),
        None => (value, false),
    };
    if space.is_empty() || space.contains(':') || space == "*" {
        return Err("Use SPACE or SPACE:rw, or --all for every space".into());
    }
    Ok(Grant {
        space: space.into(),
        write,
    })
}

fn revoke(accounts: &Accounts, account: &str, name: &str, out: &mut impl Write) -> Result<()> {
    let connections = accounts.connections(account)?;
    let mut matches = connections
        .iter()
        .filter(|connection| connection.name == name);
    let connection = matches.next().ok_or("Connection not found")?;
    if matches.next().is_some() {
        return Err("More than one connection has that name".into());
    }
    accounts.connection_revoke(account, &connection.id)?;
    writeln!(out, "Revoked connection {}", name.escape_debug())?;
    Ok(())
}

#[derive(Subcommand)]
pub(crate) enum SpaceCommand {
    /// Create a space, optionally beneath a parent space.
    Create {
        #[arg(long, value_name = "ID")]
        account: String,
        name: String,
        label: String,
        #[arg(long, value_name = "P")]
        parent: Option<String>,
    },
}

impl SpaceCommand {
    pub(crate) fn run(self, accounts: &Accounts, out: &mut impl Write) -> Result<()> {
        let Self::Create {
            account,
            name,
            label,
            parent,
        } = self;
        let account = accounts.account(&account)?;
        account
            .store
            .space_create(&name, &label, parent.as_deref())?;
        writeln!(out, "Created space {name}")?;
        Ok(())
    }
}

pub(crate) fn check(accounts: &Accounts, fix: bool, out: &mut impl Write) -> Result<()> {
    let mut failures = Vec::new();
    for (id, _) in accounts.account_list()? {
        let report = match accounts
            .account(&id)
            .and_then(|account| account.store.sweep_blobs(fix).map_err(AccessError::from))
        {
            Ok(report) => report,
            Err(error) => {
                failures.push(format!("{id}\t{error}"));
                continue;
            }
        };
        if report.orphans.is_empty() && report.temporary.is_empty() {
            writeln!(out, "{id}\tclean")?;
        }
        for (kind, paths) in [
            ("orphan blob", report.orphans),
            ("temporary file", report.temporary),
        ] {
            let action = if fix { "removed " } else { "" };
            for path in paths {
                writeln!(out, "{id}\t{action}{kind}\t{:?}", path)?;
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n").into())
    }
}
