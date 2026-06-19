mod manifest;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

/// Manage agent skills across deployments.
///
/// Syncs skills from external git submodule sources into a bundle directory,
/// with 3-way merge support for locally-modified skills.
#[derive(Parser)]
#[command(name = "skillz", version, about)]
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
            let repo_root = find_repo_root()?;
            let _manifest = manifest::Manifest::load(&repo_root)?;
            println!("sync: manifest loaded, bundle sync not yet implemented");
        }
        Commands::Init => {
            println!("init: not yet implemented");
        }
    }

    Ok(())
}

/// Find the repository root by searching for a `.git` directory or file.
fn find_repo_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("failed to get current directory")?;
    let mut current = cwd.as_path();
    loop {
        if current.join(".git").exists() {
            return Ok(current.to_path_buf());
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => bail!("not inside a git repository — could not find .git directory"),
        }
    }
}
