mod admin;

use admin::{AccountCommand, KeyCommand, SpaceCommand};
use clap::error::ErrorKind;
use clap::{Args, ColorChoice, Parser, Subcommand};
use pensine_access::Accounts;
use pensine_server::{serve_blocking, Config};
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(name = "pensine", version, about = "A memory for assistants", color = ColorChoice::Never)]
struct Cli {
    /// Directory holding the data (accounts, stores, blobs).
    #[arg(long, global = true, default_value = "/data", value_name = "DIR")]
    data: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the server (MCP, OAuth, and admin listeners) and run until interrupted.
    Serve(ServeArgs),
    /// Print the installed version.
    Version,
    /// Export an account's notes and attachments.
    Export {
        #[arg(long, value_name = "ID")]
        account: String,
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
    },
    /// Manage accounts and their public aliases.
    #[command(subcommand)]
    Account(AccountCommand),
    /// Manage bearer keys.
    #[command(subcommand)]
    Key(KeyCommand),
    /// Manage an account's spaces.
    #[command(subcommand)]
    Space(SpaceCommand),
    /// Report orphan blobs and temporary upload files in every account.
    Check {
        /// Remove the orphan and temporary files found.
        #[arg(long)]
        fix: bool,
    },
}

#[derive(Args)]
struct ServeArgs {
    /// Optional TOML configuration file.
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let display = matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            );
            return if error.print().is_ok() && display {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
    };
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("pensine: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let accounts = || Accounts::open(&cli.data);
    match cli.command {
        Command::Serve(args) => {
            let mut config = match args.config {
                Some(path) => Config::from_file(&path)?,
                None => Config::default(),
            };
            config.data_dir = cli.data;
            // Request lines only (method, path, status, duration): never a body, a key or a note.
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::INFO)
                .with_ansi(false)
                .with_target(false)
                .init();
            Ok(serve_blocking(config)?)
        }
        Command::Version => {
            writeln!(io::stdout().lock(), "pensine {}", env!("CARGO_PKG_VERSION"))?;
            Ok(())
        }
        Command::Account(command) => command.run(&accounts()?, &mut io::stdout().lock()),
        Command::Key(command) => command.run(&accounts()?, &mut io::stdout().lock()),
        Command::Space(command) => command.run(&accounts()?, &mut io::stdout().lock()),
        Command::Export { account, out } => {
            accounts()?.account(&account)?.store.export(&out)?;
            writeln!(io::stdout().lock(), "Exported account {account}")?;
            Ok(())
        }
        Command::Check { fix } => admin::check(&accounts()?, fix, &mut io::stdout().lock()),
    }
}
