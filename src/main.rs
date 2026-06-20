mod git;
mod manifest;
mod sync;

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
    Sync {
        /// Optional source name to query the current submodule HEAD SHA.
        /// When provided, prints the current ref for that source instead of syncing.
        #[arg(long)]
        source: Option<String>,
    },
    /// Create a minimal manifest and directory structure
    Init,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Sync { source } => {
            let repo_root = find_repo_root()?;
            let manifest = manifest::Manifest::load(&repo_root)?;
            match source {
                Some(name) => {
                    let sha = git::get_submodule_head_sha(&repo_root, &name)?;
                    println!("{name}: {sha}");
                }
                None => {
                    let result = sync::sync_unmodified(&repo_root, &manifest)?;
                    println!(
                        "synced {} skills, {} unchanged",
                        result.synced.len(),
                        result.unchanged.len()
                    );
                }
            }
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
