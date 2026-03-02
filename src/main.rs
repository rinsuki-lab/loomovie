mod cmd;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "loomovie")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Merge fragmented MP4 streams into a single file
    Generate {
        /// Path to streams.json input file
        streams_json: String,
        /// Output prefix (e.g., "output/out" produces output/out.init.m4s, etc.)
        prefix: String,
    },
    /// Validate a generated MP4 against its sources.json
    Validate {
        /// Path to the sources.json file
        sources_json: String,
        /// Path to the combined MP4 file (cat prefix.init.m4s prefix.data.m4s)
        mp4: String,
    },
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Generate {
            streams_json,
            prefix,
        } => {
            cmd::generate::run(&streams_json, &prefix);
        }
        Commands::Validate { sources_json, mp4 } => {
            cmd::validate::run(&sources_json, &mp4);
        }
    }
}
