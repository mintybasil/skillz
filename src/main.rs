mod git;
mod manifest;
mod report;
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
            match source {
                Some(name) => {
                    let sha = git::get_submodule_head_sha(&repo_root, &name)?;
                    println!("{name}: {sha}");
                }
                None => {
                    let mut manifest = manifest::Manifest::load(&repo_root)?;
                    let mut result = sync::sync_unmodified(&repo_root, &manifest)?;
                    let modified_result = sync::sync_modified(&repo_root, &mut manifest)?;

                    // Check if any base_refs were updated before moving fields
                    let need_save = modified_result.any_base_ref_updated();

                    // Merge modified results into the main result
                    result.merged = modified_result.merged;
                    result.up_to_date = modified_result.up_to_date;
                    result.conflicted = modified_result.conflicted;
                    result.drift_warnings = modified_result.drift_warnings;

                    // Save manifest if any base_refs were updated
                    if need_save {
                        manifest.save(&repo_root)?;
                    }

                    // Print structured sync report
                    report::print_sync_report(&result, &repo_root)?;

                    // Exit with non-zero code if any conflicts occurred
                    if !result.conflicted.is_empty() {
                        std::process::exit(1);
                    }
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
