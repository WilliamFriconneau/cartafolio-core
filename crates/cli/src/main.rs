use clap::{Args, Parser, Subcommand};
use pensine_server::{serve_blocking, Config};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "pensine", version, about = "A memory for assistants")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the server (MCP, OAuth, and admin listeners) and run until interrupted.
    Serve(ServeArgs),
}

#[derive(Args)]
struct ServeArgs {
    /// Directory holding the data (accounts, stores, blobs).
    #[arg(long, default_value = "/data")]
    data: PathBuf,
    /// Optional TOML configuration file.
    #[arg(long)]
    config: Option<PathBuf>,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve(args) => run_serve(args),
    }
}

fn run_serve(args: ServeArgs) {
    let mut config = match &args.config {
        Some(path) => match Config::from_file(path) {
            Ok(config) => config,
            Err(error) => {
                eprintln!("pensine: {error}");
                std::process::exit(1);
            }
        },
        None => Config::default(),
    };
    config.data_dir = args.data;
    if let Err(error) = serve_blocking(config) {
        eprintln!("pensine: {error}");
        std::process::exit(1);
    }
}
