//! Sync report generation.
//!
//! Produces a structured, human-readable summary of a sync operation.
//! The report includes sections for synced, unchanged, merged, up-to-date,
//! and conflicted skills, plus drift warnings and local skill counts.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use crate::sync::SyncResult;

/// Count the number of local skill directories under `skills/mintybasil/`.
///
/// Each immediate subdirectory of `skills/mintybasil/` is counted as one skill.
/// Returns 0 if the directory does not exist.
pub fn count_local_skills(repo_root: &Path) -> Result<usize> {
    let local_dir = repo_root.join("skills").join("mintybasil");
    if !local_dir.exists() {
        return Ok(0);
    }
    let mut count = 0usize;
    for entry in fs::read_dir(&local_dir).with_context(|| {
        format!(
            "failed to read local skills directory: {}",
            local_dir.display()
        )
    })? {
        let entry = entry.with_context(|| {
            format!(
                "failed to read entry in local skills directory: {}",
                local_dir.display()
            )
        })?;
        if entry.file_type()?.is_dir() {
            count += 1;
        }
    }
    Ok(count)
}

/// Truncate a SHA string to its first 8 characters for display.
fn short_sha(sha: &str) -> &str {
    if sha.len() >= 8 {
        &sha[..8]
    } else {
        sha
    }
}

/// Write the sync report to the given writer.
///
/// This is the core report function. `print_sync_report` wraps it by
/// writing to stdout.
pub fn write_sync_report<W: Write>(
    result: &SyncResult,
    repo_root: &Path,
    writer: &mut W,
) -> Result<()> {
    writeln!(writer, "=== Sync Complete ===")?;
    writeln!(writer)?;

    // Synced (copied from upstream)
    if !result.synced.is_empty() {
        writeln!(writer, "Synced (copied from upstream):")?;
        for (source, path) in &result.synced {
            writeln!(writer, "  {source}/{path}")?;
        }
        writeln!(writer)?;
    }

    // Unchanged (already up to date)
    if !result.unchanged.is_empty() {
        writeln!(writer, "Unchanged (already up to date):")?;
        writeln!(writer, "  {} skills", result.unchanged.len())?;
        writeln!(writer)?;
    }

    // Merged (upstream changes applied)
    if !result.merged.is_empty() {
        writeln!(writer, "Merged (upstream changes applied):")?;
        for m in &result.merged {
            writeln!(writer, "  {}/{}", m.source_name, m.skill_path)?;
            writeln!(
                writer,
                "    ref: {} \u{2192} {}",
                short_sha(&m.old_ref),
                short_sha(&m.new_ref)
            )?;
            if !m.files_merged.is_empty() {
                writeln!(writer, "    files merged: {}", m.files_merged.join(", "))?;
            }
        }
        writeln!(writer)?;
    }

    // Up to date (modified, no upstream change)
    if !result.up_to_date.is_empty() {
        writeln!(writer, "Up to date (modified, no upstream change):")?;
        writeln!(
            writer,
            "  {} skill{}",
            result.up_to_date.len(),
            if result.up_to_date.len() == 1 {
                ""
            } else {
                "s"
            }
        )?;
        writeln!(writer)?;
    }

    // Local skills (mintybasil/)
    let local_count = count_local_skills(repo_root)?;
    if local_count > 0 {
        writeln!(writer, "Local skills (mintybasil/):")?;
        writeln!(
            writer,
            "  {} skill{}",
            local_count,
            if local_count == 1 { "" } else { "s" }
        )?;
        writeln!(writer)?;
    }

    // Drift warnings
    if !result.drift_warnings.is_empty() {
        for (source_name, skill_path, ref_) in &result.drift_warnings {
            writeln!(
                writer,
                "WARNING: ref '{}' for skill '{}' in source '{}'",
                ref_, skill_path, source_name
            )?;
            writeln!(
                writer,
                "could not be resolved in the submodule \u{2014} the tag/branch may have been deleted upstream."
            )?;
        }
        writeln!(writer)?;
    }

    // Conflicts (ACTION REQUIRED)
    if !result.conflicted.is_empty() {
        let total_conflict_files: usize = result
            .conflicted
            .iter()
            .map(|m| m.files_conflicted.len())
            .sum();
        writeln!(writer, "=== ACTION REQUIRED ===")?;
        writeln!(writer)?;
        writeln!(
            writer,
            "Conflicts ({} files in {} skill{}):",
            total_conflict_files,
            result.conflicted.len(),
            if result.conflicted.len() == 1 {
                ""
            } else {
                "s"
            }
        )?;
        for m in &result.conflicted {
            writeln!(writer, "  {}/{}", m.source_name, m.skill_path)?;
            for file in &m.files_conflicted {
                writeln!(writer, "    CONFLICT: {file}")?;
            }
            writeln!(writer)?;
        }
        writeln!(
            writer,
            "  Resolve conflict markers in the files above, then update ref"
        )?;
        writeln!(
            writer,
            "  in skills-manifest.yaml to the current submodule HEAD SHA."
        )?;
        writeln!(writer)?;
    }

    // Manifest save status
    if result.any_ref_updated() {
        let updated_count = result
            .merged
            .iter()
            .filter(|m| m.new_ref != m.old_ref)
            .count();
        writeln!(
            writer,
            "Manifest saved: ref updated for {} skill{}.",
            updated_count,
            if updated_count == 1 { "" } else { "s" }
        )?;
    }

    Ok(())
}

/// Print the sync report to stdout.
pub fn print_sync_report(result: &SyncResult, repo_root: &Path) -> Result<()> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    write_sync_report(result, repo_root, &mut lock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{SkillEntry, Source};
    use crate::sync::MergeResult;
    use std::fs;
    use tempfile::TempDir;

    /// Build a SyncResult for testing.
    fn make_result() -> SyncResult {
        SyncResult::default()
    }

    /// Helper to capture report output as a String.
    fn capture_report(result: &SyncResult, repo_root: &Path) -> String {
        let mut buf: Vec<u8> = Vec::new();
        write_sync_report(result, repo_root, &mut buf).expect("write_sync_report failed");
        String::from_utf8(buf).expect("report output was not UTF-8")
    }

    // ── Local skill counting tests ───────────────────────────────────

    #[test]
    fn test_count_local_skills_empty() {
        let dir = TempDir::new().unwrap();
        // No skills/mintybasil directory at all
        assert_eq!(count_local_skills(dir.path()).unwrap(), 0);
    }

    #[test]
    fn test_count_local_skills_with_dirs() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("skills/mintybasil/skill-a")).unwrap();
        fs::create_dir_all(root.join("skills/mintybasil/skill-b")).unwrap();
        fs::create_dir_all(root.join("skills/mintybasil/skill-c")).unwrap();
        // Add a file — should not be counted as a skill
        fs::write(root.join("skills/mintybasil/README.md"), "not a dir\n").unwrap();
        assert_eq!(count_local_skills(root).unwrap(), 3);
    }

    #[test]
    fn test_count_local_skills_no_dirs() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("skills/mintybasil")).unwrap();
        fs::write(root.join("skills/mintybasil/notes.txt"), "just a file\n").unwrap();
        assert_eq!(count_local_skills(root).unwrap(), 0);
    }

    // ── Report output tests ──────────────────────────────────────────

    #[test]
    fn test_clean_sync_no_action_required() {
        let dir = TempDir::new().unwrap();
        let result = make_result();
        let output = capture_report(&result, dir.path());
        assert!(output.contains("=== Sync Complete ==="));
        assert!(!output.contains("=== ACTION REQUIRED ==="));
        assert!(!output.contains("CONFLICT"));
        assert!(!output.contains("WARNING"));
    }

    #[test]
    fn test_report_synced_section() {
        let dir = TempDir::new().unwrap();
        let mut result = make_result();
        result.synced.push((
            "hermes-skills".to_string(),
            "devops/kanban-orchestrator".to_string(),
        ));
        result.synced.push((
            "hermes-skills".to_string(),
            "productivity/notion".to_string(),
        ));
        let output = capture_report(&result, dir.path());
        assert!(output.contains("Synced (copied from upstream):"));
        assert!(output.contains("hermes-skills/devops/kanban-orchestrator"));
        assert!(output.contains("hermes-skills/productivity/notion"));
    }

    #[test]
    fn test_report_unchanged_section() {
        let dir = TempDir::new().unwrap();
        let mut result = make_result();
        result
            .unchanged
            .push(("hermes-skills".to_string(), "skill-a".to_string()));
        result
            .unchanged
            .push(("hermes-skills".to_string(), "skill-b".to_string()));
        let output = capture_report(&result, dir.path());
        assert!(output.contains("Unchanged (already up to date):"));
        assert!(output.contains("2 skills"));
    }

    #[test]
    fn test_report_merged_section() {
        let dir = TempDir::new().unwrap();
        let mut result = make_result();
        result.merged.push(MergeResult {
            source_name: "hermes-skills".to_string(),
            skill_path: "mlops/inference/llama-cpp".to_string(),
            old_ref: "abc123def456".to_string(),
            new_ref: "def456abc789".to_string(),
            files_merged: vec!["SKILL.md".to_string(), "references/api.md".to_string()],
            files_conflicted: vec![],
        });
        let output = capture_report(&result, dir.path());
        assert!(output.contains("Merged (upstream changes applied):"));
        assert!(output.contains("hermes-skills/mlops/inference/llama-cpp"));
        assert!(output.contains("ref: abc123de \u{2192} def456ab"));
        assert!(output.contains("files merged: SKILL.md, references/api.md"));
    }

    #[test]
    fn test_report_up_to_date_section() {
        let dir = TempDir::new().unwrap();
        let mut result = make_result();
        result
            .up_to_date
            .push(("hermes-skills".to_string(), "some-skill".to_string()));
        let output = capture_report(&result, dir.path());
        assert!(output.contains("Up to date (modified, no upstream change):"));
        assert!(output.contains("1 skill"));
    }

    #[test]
    fn test_report_up_to_date_plural() {
        let dir = TempDir::new().unwrap();
        let mut result = make_result();
        result
            .up_to_date
            .push(("hermes-skills".to_string(), "skill-a".to_string()));
        result
            .up_to_date
            .push(("hermes-skills".to_string(), "skill-b".to_string()));
        let output = capture_report(&result, dir.path());
        assert!(output.contains("2 skills"));
    }

    #[test]
    fn test_report_local_skills_section() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("skills/mintybasil/local-a")).unwrap();
        fs::create_dir_all(root.join("skills/mintybasil/local-b")).unwrap();
        fs::create_dir_all(root.join("skills/mintybasil/local-c")).unwrap();
        let result = make_result();
        let output = capture_report(&result, root);
        assert!(output.contains("Local skills (mintybasil/):"));
        assert!(output.contains("3 skills"));
    }

    #[test]
    fn test_report_conflicts_section() {
        let dir = TempDir::new().unwrap();
        let mut result = make_result();
        result.conflicted.push(MergeResult {
            source_name: "hermes-skills".to_string(),
            skill_path: "mlops/models/segment-anything-model".to_string(),
            old_ref: "abc123".to_string(),
            new_ref: "abc123".to_string(),
            files_merged: vec![],
            files_conflicted: vec!["SKILL.md".to_string(), "scripts/validate.py".to_string()],
        });
        let output = capture_report(&result, dir.path());
        assert!(output.contains("=== ACTION REQUIRED ==="));
        assert!(output.contains("Conflicts (2 files in 1 skill):"));
        assert!(output.contains("hermes-skills/mlops/models/segment-anything-model"));
        assert!(output.contains("CONFLICT: SKILL.md"));
        assert!(output.contains("CONFLICT: scripts/validate.py"));
        assert!(output.contains("Resolve conflict markers"));
        assert!(output.contains("skills-manifest.yaml"));
    }

    #[test]
    fn test_report_multiple_conflicted_skills() {
        let dir = TempDir::new().unwrap();
        let mut result = make_result();
        result.conflicted.push(MergeResult {
            source_name: "hermes-skills".to_string(),
            skill_path: "skill-a".to_string(),
            old_ref: "abc123".to_string(),
            new_ref: "abc123".to_string(),
            files_merged: vec![],
            files_conflicted: vec!["file1.txt".to_string()],
        });
        result.conflicted.push(MergeResult {
            source_name: "hermes-skills".to_string(),
            skill_path: "skill-b".to_string(),
            old_ref: "def456".to_string(),
            new_ref: "def456".to_string(),
            files_merged: vec![],
            files_conflicted: vec!["file2.txt".to_string(), "file3.txt".to_string()],
        });
        let output = capture_report(&result, dir.path());
        assert!(output.contains("Conflicts (3 files in 2 skills):"));
        assert!(output.contains("CONFLICT: file1.txt"));
        assert!(output.contains("CONFLICT: file2.txt"));
        assert!(output.contains("CONFLICT: file3.txt"));
    }

    #[test]
    fn test_report_drift_warning() {
        let dir = TempDir::new().unwrap();
        let mut result = make_result();
        result.drift_warnings.push((
            "hermes-skills".to_string(),
            "mlops/inference/llama-cpp".to_string(),
            "v1.2.3".to_string(),
        ));
        let output = capture_report(&result, dir.path());
        assert!(output.contains(
            "WARNING: ref 'v1.2.3' for skill 'mlops/inference/llama-cpp' in source 'hermes-skills'"
        ));
        assert!(output.contains("could not be resolved in the submodule"));
        assert!(output.contains("tag/branch may have been deleted upstream"));
    }

    #[test]
    fn test_report_manifest_saved() {
        let dir = TempDir::new().unwrap();
        let mut result = make_result();
        result.merged.push(MergeResult {
            source_name: "hermes-skills".to_string(),
            skill_path: "some-skill".to_string(),
            old_ref: "old1234567890".to_string(),
            new_ref: "new1234567890".to_string(),
            files_merged: vec!["SKILL.md".to_string()],
            files_conflicted: vec![],
        });
        let output = capture_report(&result, dir.path());
        assert!(output.contains("Manifest saved: ref updated for 1 skill."));
    }

    #[test]
    fn test_report_manifest_not_saved_no_ref_change() {
        let dir = TempDir::new().unwrap();
        let mut result = make_result();
        result.merged.push(MergeResult {
            source_name: "hermes-skills".to_string(),
            skill_path: "some-skill".to_string(),
            old_ref: "same123456789".to_string(),
            new_ref: "same123456789".to_string(),
            files_merged: vec![],
            files_conflicted: vec![],
        });
        let output = capture_report(&result, dir.path());
        assert!(!output.contains("Manifest saved"));
    }

    #[test]
    fn test_report_manifest_saved_multiple_skills() {
        let dir = TempDir::new().unwrap();
        let mut result = make_result();
        result.merged.push(MergeResult {
            source_name: "hermes-skills".to_string(),
            skill_path: "skill-a".to_string(),
            old_ref: "old1".to_string(),
            new_ref: "new1".to_string(),
            files_merged: vec!["file.txt".to_string()],
            files_conflicted: vec![],
        });
        result.merged.push(MergeResult {
            source_name: "hermes-skills".to_string(),
            skill_path: "skill-b".to_string(),
            old_ref: "old2".to_string(),
            new_ref: "new2".to_string(),
            files_merged: vec!["file.txt".to_string()],
            files_conflicted: vec![],
        });
        let output = capture_report(&result, dir.path());
        assert!(output.contains("Manifest saved: ref updated for 2 skills."));
    }

    #[test]
    fn test_short_sha() {
        assert_eq!(short_sha("abcdef1234567890"), "abcdef12");
        assert_eq!(short_sha("short"), "short");
        assert_eq!(short_sha(""), "");
    }

    // ── Integration: full report with all sections ───────────────────

    #[test]
    fn test_full_report_all_sections() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        // Create local skills
        fs::create_dir_all(root.join("skills/mintybasil/local-a")).unwrap();
        fs::create_dir_all(root.join("skills/mintybasil/local-b")).unwrap();

        let mut result = make_result();
        result
            .synced
            .push(("hermes-skills".to_string(), "devops/kanban".to_string()));
        result
            .unchanged
            .push(("hermes-skills".to_string(), "unchanged-skill".to_string()));
        result.merged.push(MergeResult {
            source_name: "hermes-skills".to_string(),
            skill_path: "mlops/llama".to_string(),
            old_ref: "aaa111222333".to_string(),
            new_ref: "bbb444555666".to_string(),
            files_merged: vec!["SKILL.md".to_string()],
            files_conflicted: vec![],
        });
        result.up_to_date.push((
            "hermes-skills".to_string(),
            "modified-no-change".to_string(),
        ));
        result.conflicted.push(MergeResult {
            source_name: "hermes-skills".to_string(),
            skill_path: "conflicted-skill".to_string(),
            old_ref: "ccc777".to_string(),
            new_ref: "ccc777".to_string(),
            files_merged: vec![],
            files_conflicted: vec!["file.txt".to_string()],
        });
        result.drift_warnings.push((
            "hermes-skills".to_string(),
            "drift-skill".to_string(),
            "v1.0.0".to_string(),
        ));

        let output = capture_report(&result, root);

        // Check all sections present
        assert!(output.contains("=== Sync Complete ==="));
        assert!(output.contains("Synced (copied from upstream):"));
        assert!(output.contains("Unchanged (already up to date):"));
        assert!(output.contains("Merged (upstream changes applied):"));
        assert!(output.contains("Up to date (modified, no upstream change):"));
        assert!(output.contains("Local skills (mintybasil/):"));
        assert!(output.contains("WARNING: ref 'v1.0.0'"));
        assert!(output.contains("=== ACTION REQUIRED ==="));
        assert!(output.contains("Manifest saved: ref updated for 1 skill."));
    }

    // ── Drift warning in sync_modified integration test ──────────────

    #[test]
    fn test_sync_modified_drift_warning_unresolvable_ref() {
        use crate::sync::sync_modified;
        use std::process::Command as StdCommand;

        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let source = "src1";
        let skill = "my-skill";

        // Create repo structure
        fs::create_dir_all(root.join("sources").join(source).join(skill)).unwrap();
        fs::create_dir_all(root.join("skills")).unwrap();

        // Init git repo in sources/<source>
        let source_repo = root.join("sources").join(source);
        StdCommand::new("git")
            .arg("-C")
            .arg(&source_repo)
            .args(["init"])
            .status()
            .expect("git init failed");
        StdCommand::new("git")
            .arg("-C")
            .arg(&source_repo)
            .args(["config", "user.email", "test@example.com"])
            .status()
            .expect("git config email failed");
        StdCommand::new("git")
            .arg("-C")
            .arg(&source_repo)
            .args(["config", "user.name", "Test"])
            .status()
            .expect("git config name failed");

        // Commit something so HEAD exists
        fs::write(source_repo.join(skill).join("file.txt"), "content\n").unwrap();
        StdCommand::new("git")
            .arg("-C")
            .arg(&source_repo)
            .args(["add", "-A"])
            .status()
            .expect("git add failed");
        StdCommand::new("git")
            .arg("-C")
            .arg(&source_repo)
            .args(["commit", "-m", "initial"])
            .status()
            .expect("git commit failed");

        // Use a ref that doesn't exist (deleted tag)
        let mut manifest = crate::manifest::Manifest {
            sources: vec![Source {
                name: "src1".to_string(),
                ref_: Some("refs/tags/deleted-tag".to_string()),
                base_path: None,
                skills: vec![SkillEntry::modified("my-skill")],
            }],
        };

        let result = sync_modified(root, &mut manifest).unwrap();

        // Should have a drift warning, not an error
        assert_eq!(result.drift_warnings.len(), 1);
        assert_eq!(result.drift_warnings[0].0, "src1");
        assert_eq!(result.drift_warnings[0].1, "my-skill");
        assert_eq!(result.drift_warnings[0].2, "refs/tags/deleted-tag");

        // Should NOT have merged or conflicted anything
        assert_eq!(result.merged.len(), 0);
        assert_eq!(result.conflicted.len(), 0);
        assert_eq!(result.up_to_date.len(), 0);
    }
}
