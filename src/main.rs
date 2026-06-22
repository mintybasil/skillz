mod commands;
mod git;
mod manifest;
mod report;
mod sync;
mod tui;

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
    /// Import skills from a path into the manifest
    Import {
        /// Path to scan for skills (e.g., "hermes-agent/skills")
        path: String,
    },
    /// Validate the manifest and check for common issues
    Lint,
    /// Create a minimal manifest and directory structure
    Init,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let repo_root = find_repo_root()?;

    match cli.command {
        Commands::Sync { source } => match source {
            Some(name) => {
                let sha = git::get_submodule_head_sha(&repo_root, &name)?;
                println!("{name}: {sha}");
            }
            None => {
                let mut manifest = manifest::Manifest::load(&repo_root)?;
                let mut result = sync::sync_unmodified(&repo_root, &manifest)?;
                let modified_result = sync::sync_modified(&repo_root, &mut manifest)?;

                let need_save = modified_result.any_ref_updated();

                result.merged = modified_result.merged;
                result.up_to_date = modified_result.up_to_date;
                result.conflicted = modified_result.conflicted;
                result.drift_warnings = modified_result.drift_warnings;

                if need_save {
                    manifest.save(&repo_root)?;
                }

                report::print_sync_report(&result, &repo_root)?;

                if !result.conflicted.is_empty() {
                    std::process::exit(1);
                }
            }
        },
        Commands::Import { path } => {
            commands::run_import(&repo_root, &path)?;
        }
        Commands::Lint => {
            commands::run_lint(&repo_root)?;
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
