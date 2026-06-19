use anyhow::Result;
use clap::{Parser, Subcommand};

/// Manage agent skills across deployments.
///
/// Syncs skills from external git submodule sources into a bundle directory,
/// with 3-way merge support for locally-modified skills.
#[derive(Parser)]
#[command(name = "skills-managed", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build or rebuild the skill bundle from submodules + manifest
    Sync,
    /// Create a minimal manifest and directory structure
    Init,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Sync => {
            println!("sync: not yet implemented");
        }
        Commands::Init => {
            println!("init: not yet implemented");
        }
    }

    Ok(())
}
