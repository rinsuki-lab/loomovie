mod cmd;
mod proto;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "loomovie")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate a recipe.pb describing how to assemble a Hybrid MP4 from fragmented streams
    Plan {
        /// Path to streams.json input file
        streams_json: String,
        /// Output path for the recipe file (e.g., "output/recipe.pb")
        recipe_pb: String,
    },
    /// Output binary data from a recipe.pb for a given byte range
    Bin {
        /// Path to the recipe.pb file
        recipe_pb: String,
        /// Start byte offset (inclusive), defaults to 0
        start: Option<u64>,
        /// End byte offset (exclusive), defaults to end of file
        end: Option<u64>,
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
        Commands::Plan {
            streams_json,
            recipe_pb,
        } => {
            cmd::plan::run(&streams_json, &recipe_pb);
        }
        Commands::Bin {
            recipe_pb,
            start,
            end,
        } => {
            cmd::bin::run(&recipe_pb, start, end);
        }
    }
}
