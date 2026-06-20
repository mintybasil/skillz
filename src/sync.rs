//! Sync unmodified skills from git submodules into the bundle directory.
//!
//! For each source in the manifest, for each skill where `modified: false`:
//! - Source: `sources/<source_name>/<skill_path>`
//! - Destination: `skills/<source_name>/<skill_path>`
//!
//! Uses a clear-then-copy strategy: the destination skill directory is removed
//! entirely before copying fresh. This ensures files removed upstream are
//! cleaned from the bundle.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use walkdir::WalkDir;

use crate::manifest::{Manifest, SkillEntry, Source};

/// Result of syncing unmodified skills.
#[derive(Debug, Clone, Default)]
pub struct SyncResult {
    /// Skills that were copied (source_name, skill_path).
    pub synced: Vec<(String, String)>,
    /// Skills that were already up to date (source_name, skill_path).
    pub unchanged: Vec<(String, String)>,
}

/// Sync all unmodified skills from submodules into the bundle directory.
///
/// Iterates over every source in the manifest, and for each skill where
/// `modified: false`, copies the skill directory from `sources/<source>/<path>`
/// to `skills/<source>/<path>` using a clear-then-copy strategy. Skills that
/// are already byte-for-byte identical are skipped to reduce git diff noise.
pub fn sync_unmodified(repo_root: &Path, manifest: &Manifest) -> Result<SyncResult> {
    let sources_root = repo_root.join("sources");
    let bundle_root = repo_root.join("skills");

    let mut result = SyncResult::default();

    for source in &manifest.sources {
        for skill in &source.skills {
            if skill.modified {
                continue;
            }

            let src_path = sources_root.join(&source.name).join(&skill.path);
            let dst_path = bundle_root.join(&source.name).join(&skill.path);

            if !src_path.exists() {
                anyhow::bail!(
                    "source skill directory does not exist: {} (for source '{}', skill '{}')",
                    src_path.display(),
                    source.name,
                    skill.path
                );
            }

            if dir_contents_equal(&src_path, &dst_path)? {
                result
                    .unchanged
                    .push((source.name.clone(), skill.path.clone()));
            } else {
                copy_skill_dir(&src_path, &dst_path)?;
                result
                    .synced
                    .push((source.name.clone(), skill.path.clone()));
            }
        }
    }

    Ok(result)
}

/// Remove the destination skill directory if it exists, then recursively copy
/// the source directory into its place.
fn copy_skill_dir(src: &Path, dst: &Path) -> Result<()> {
    // Clear-then-copy: remove destination entirely to handle upstream deletes
    if dst.exists() {
        fs::remove_dir_all(dst)
            .with_context(|| format!("failed to remove existing destination: {}", dst.display()))?;
    }

    // Create parent directories
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent dirs: {}", parent.display()))?;
    }

    // Copy recursively using walkdir
    for entry in WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
        let relative = entry
            .path()
            .strip_prefix(src)
            .expect("walkdir entry is under src");
        let target = dst.join(relative);

        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)
                .with_context(|| format!("failed to create directory: {}", target.display()))?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create parent for file: {}", parent.display())
                })?;
            }
            fs::copy(entry.path(), &target).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    entry.path().display(),
                    target.display()
                )
            })?;
        }
        // Symlinks and other file types are intentionally skipped for safety
    }

    Ok(())
}

/// Compare two directories by content. Returns `true` if they have identical
/// file trees (same relative paths and same file contents).
///
/// If the destination does not exist, returns `false`.
/// If both directories are empty, returns `true`.
fn dir_contents_equal(src: &Path, dst: &Path) -> Result<bool> {
    if !dst.exists() {
        return Ok(false);
    }

    let src_files = collect_file_map(src)?;
    let dst_files = collect_file_map(dst)?;

    if src_files.len() != dst_files.len() {
        return Ok(false);
    }

    for (rel_path, src_bytes) in &src_files {
        match dst_files.get(rel_path) {
            Some(dst_bytes) if src_bytes == dst_bytes => {}
            _ => return Ok(false),
        }
    }

    Ok(true)
}

/// Collect all regular files under a directory into a map of
/// (relative_path -> file_contents).
fn collect_file_map(dir: &Path) -> Result<HashMap<PathBuf, Vec<u8>>> {
    let mut files = HashMap::new();

    if !dir.exists() {
        return Ok(files);
    }

    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            let rel = entry
                .path()
                .strip_prefix(dir)
                .expect("walkdir entry is under dir")
                .to_path_buf();
            let mut buf = Vec::new();
            let mut f = fs::File::open(entry.path())
                .with_context(|| format!("failed to open file: {}", entry.path().display()))?;
            f.read_to_end(&mut buf)
                .with_context(|| format!("failed to read file: {}", entry.path().display()))?;
            files.insert(rel, buf);
        }
    }

    Ok(files)
}

/// Helper trait/impl for tests to build a Source easily.
#[allow(dead_code)]
impl Source {
    /// Create a new source with the given name and no skills.
    pub fn new(name: &str) -> Self {
        Source {
            name: name.to_string(),
            skills: Vec::new(),
        }
    }
}

/// Helper for tests to build a SkillEntry easily.
#[allow(dead_code)]
impl SkillEntry {
    /// Create an unmodified skill entry at the given path.
    pub fn unmodified(path: &str) -> Self {
        SkillEntry {
            path: path.to_string(),
            modified: false,
            base_ref: None,
            mod_tags: None,
        }
    }

    /// Create a modified skill entry at the given path with the given base_ref.
    pub fn modified(path: &str, base_ref: &str) -> Self {
        SkillEntry {
            path: path.to_string(),
            modified: true,
            base_ref: Some(base_ref.to_string()),
            mod_tags: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    // ── Test helpers ──────────────────────────────────────────────────

    /// Build a fake repo structure in a temp dir.
    ///
    /// Creates:
    /// ```text
    /// <root>/
    ///   sources/
    ///     <source_name>/
    ///       <skill_path>/  (with the given files)
    ///   skills/            (empty, created on demand)
    ///   skills-manifest.yaml  (with the given manifest content)
    /// ```
    fn build_repo(source_name: &str) -> TempDir {
        let dir = TempDir::new().expect("failed to create temp dir");
        let root = dir.path();

        fs::create_dir_all(root.join("sources").join(source_name)).unwrap();
        fs::create_dir_all(root.join("skills")).unwrap();

        dir
    }

    /// Write a file inside the temp repo root.
    fn write_file(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    /// Read a file from the temp repo root. Panics if missing.
    fn read_file(root: &Path, rel: &str) -> String {
        fs::read_to_string(root.join(rel))
            .unwrap_or_else(|e| panic!("failed to read {}: {}", rel, e))
    }

    /// Check if a path exists under root.
    fn exists(root: &Path, rel: &str) -> bool {
        root.join(rel).exists()
    }

    /// Create a source skill with a couple of files.
    fn setup_source_skill(root: &Path, source: &str, skill_path: &str) {
        write_file(
            root,
            &format!("sources/{source}/{skill_path}/README.md"),
            "# Test Skill\n",
        );
        write_file(
            root,
            &format!("sources/{source}/{skill_path}/skill.md"),
            "Skill content here\n",
        );
        // Nested subdirectory
        write_file(
            root,
            &format!("sources/{source}/{skill_path}/sub/nested.txt"),
            "nested content\n",
        );
    }

    fn make_manifest(sources: Vec<Source>) -> Manifest {
        Manifest { sources }
    }

    // ── Tests ──────────────────────────────────────────────────────────

    #[test]
    fn test_unmodified_skills_copied_to_bundle() {
        let dir = build_repo("hermes-skills");
        let root = dir.path();

        setup_source_skill(root, "hermes-skills", "kanban-orchestrator");

        let manifest = make_manifest(vec![Source {
            name: "hermes-skills".to_string(),
            skills: vec![SkillEntry::unmodified("kanban-orchestrator")],
        }]);

        let result = sync_unmodified(root, &manifest).unwrap();

        assert_eq!(result.synced.len(), 1);
        assert_eq!(result.unchanged.len(), 0);
        assert_eq!(result.synced[0].0, "hermes-skills");
        assert_eq!(result.synced[0].1, "kanban-orchestrator");

        // Verify files exist at the destination
        assert!(exists(
            root,
            "skills/hermes-skills/kanban-orchestrator/README.md"
        ));
        assert!(exists(
            root,
            "skills/hermes-skills/kanban-orchestrator/skill.md"
        ));
        assert!(exists(
            root,
            "skills/hermes-skills/kanban-orchestrator/sub/nested.txt"
        ));

        // Verify content
        assert_eq!(
            read_file(root, "skills/hermes-skills/kanban-orchestrator/README.md"),
            "# Test Skill\n"
        );
        assert_eq!(
            read_file(root, "skills/hermes-skills/kanban-orchestrator/skill.md"),
            "Skill content here\n"
        );
        assert_eq!(
            read_file(
                root,
                "skills/hermes-skills/kanban-orchestrator/sub/nested.txt"
            ),
            "nested content\n"
        );
    }

    #[test]
    fn test_category_nesting_preserved() {
        let dir = build_repo("hermes-skills");
        let root = dir.path();

        // Skill with nested category path: devops/kanban-orchestrator
        setup_source_skill(root, "hermes-skills", "devops/kanban-orchestrator");

        let manifest = make_manifest(vec![Source {
            name: "hermes-skills".to_string(),
            skills: vec![SkillEntry::unmodified("devops/kanban-orchestrator")],
        }]);

        let result = sync_unmodified(root, &manifest).unwrap();

        assert_eq!(result.synced.len(), 1);

        // Verify nested path structure is preserved
        assert!(exists(
            root,
            "skills/hermes-skills/devops/kanban-orchestrator/README.md"
        ));
        assert!(exists(
            root,
            "skills/hermes-skills/devops/kanban-orchestrator/skill.md"
        ));
        assert!(exists(
            root,
            "skills/hermes-skills/devops/kanban-orchestrator/sub/nested.txt"
        ));
    }

    #[test]
    fn test_rerun_sync_unchanged() {
        let dir = build_repo("hermes-skills");
        let root = dir.path();

        setup_source_skill(root, "hermes-skills", "my-skill");

        let manifest = make_manifest(vec![Source {
            name: "hermes-skills".to_string(),
            skills: vec![SkillEntry::unmodified("my-skill")],
        }]);

        // First sync
        let result1 = sync_unmodified(root, &manifest).unwrap();
        assert_eq!(result1.synced.len(), 1);
        assert_eq!(result1.unchanged.len(), 0);

        // Second sync — should detect no changes
        let result2 = sync_unmodified(root, &manifest).unwrap();
        assert_eq!(result2.synced.len(), 0);
        assert_eq!(result2.unchanged.len(), 1);
        assert_eq!(result2.unchanged[0].1, "my-skill");
    }

    #[test]
    fn test_files_removed_from_source_removed_from_bundle() {
        let dir = build_repo("hermes-skills");
        let root = dir.path();

        // Initial source skill
        setup_source_skill(root, "hermes-skills", "my-skill");

        let manifest = make_manifest(vec![Source {
            name: "hermes-skills".to_string(),
            skills: vec![SkillEntry::unmodified("my-skill")],
        }]);

        // First sync
        sync_unmodified(root, &manifest).unwrap();
        assert!(exists(root, "skills/hermes-skills/my-skill/sub/nested.txt"));

        // Remove a file from source (simulating upstream removal)
        fs::remove_file(root.join("sources/hermes-skills/my-skill/sub/nested.txt")).unwrap();

        // Second sync — clear-then-copy should remove the deleted file from bundle
        let result = sync_unmodified(root, &manifest).unwrap();
        assert_eq!(result.synced.len(), 1, "should be synced (content changed)");

        // The removed file should no longer exist in the bundle
        assert!(!exists(
            root,
            "skills/hermes-skills/my-skill/sub/nested.txt"
        ));

        // Other files should still exist
        assert!(exists(root, "skills/hermes-skills/my-skill/README.md"));
    }

    #[test]
    fn test_modified_skills_skipped() {
        let dir = build_repo("hermes-skills");
        let root = dir.path();

        // Create source dirs for both skills
        setup_source_skill(root, "hermes-skills", "unmodified-skill");
        setup_source_skill(root, "hermes-skills", "modified-skill");

        let manifest = make_manifest(vec![Source {
            name: "hermes-skills".to_string(),
            skills: vec![
                SkillEntry::unmodified("unmodified-skill"),
                SkillEntry::modified("modified-skill", "v1.0.0"),
            ],
        }]);

        let result = sync_unmodified(root, &manifest).unwrap();

        // Only the unmodified skill should be synced
        assert_eq!(result.synced.len(), 1);
        assert_eq!(result.synced[0].1, "unmodified-skill");

        // The modified skill should NOT be in the bundle
        assert!(!exists(root, "skills/hermes-skills/modified-skill"));
        // The unmodified skill should be in the bundle
        assert!(exists(root, "skills/hermes-skills/unmodified-skill"));
    }

    #[test]
    fn test_mintybasil_not_touched() {
        let dir = build_repo("hermes-skills");
        let root = dir.path();

        // Create a mintybasil directory with some content
        write_file(root, "skills/mintybasil/local-skill.md", "local content\n");
        write_file(root, "skills/mintybasil/another.md", "more local content\n");

        // Also create a source skill to sync
        setup_source_skill(root, "hermes-skills", "my-skill");

        let manifest = make_manifest(vec![Source {
            name: "hermes-skills".to_string(),
            skills: vec![SkillEntry::unmodified("my-skill")],
        }]);

        sync_unmodified(root, &manifest).unwrap();

        // mintybasil/ should be untouched
        assert!(exists(root, "skills/mintybasil/local-skill.md"));
        assert!(exists(root, "skills/mintybasil/another.md"));
        assert_eq!(
            read_file(root, "skills/mintybasil/local-skill.md"),
            "local content\n"
        );
        assert_eq!(
            read_file(root, "skills/mintybasil/another.md"),
            "more local content\n"
        );
    }

    #[test]
    fn test_multiple_sources_synced() {
        let dir = build_repo("hermes-skills");
        let root = dir.path();

        // Set up two sources
        fs::create_dir_all(root.join("sources/second-source")).unwrap();

        setup_source_skill(root, "hermes-skills", "skill-a");
        setup_source_skill(root, "second-source", "skill-b");

        let manifest = make_manifest(vec![
            Source {
                name: "hermes-skills".to_string(),
                skills: vec![SkillEntry::unmodified("skill-a")],
            },
            Source {
                name: "second-source".to_string(),
                skills: vec![SkillEntry::unmodified("skill-b")],
            },
        ]);

        let result = sync_unmodified(root, &manifest).unwrap();

        assert_eq!(result.synced.len(), 2);

        assert!(exists(root, "skills/hermes-skills/skill-a/README.md"));
        assert!(exists(root, "skills/second-source/skill-b/README.md"));
    }

    #[test]
    fn test_content_change_detected() {
        let dir = build_repo("hermes-skills");
        let root = dir.path();

        setup_source_skill(root, "hermes-skills", "my-skill");

        let manifest = make_manifest(vec![Source {
            name: "hermes-skills".to_string(),
            skills: vec![SkillEntry::unmodified("my-skill")],
        }]);

        // First sync
        sync_unmodified(root, &manifest).unwrap();

        // Modify source content
        write_file(
            root,
            "sources/hermes-skills/my-skill/README.md",
            "# Changed content\n",
        );

        // Second sync should detect the change
        let result = sync_unmodified(root, &manifest).unwrap();
        assert_eq!(result.synced.len(), 1);
        assert_eq!(result.unchanged.len(), 0);

        // Verify the change propagated
        assert_eq!(
            read_file(root, "skills/hermes-skills/my-skill/README.md"),
            "# Changed content\n"
        );
    }

    #[test]
    fn test_nonexistent_source_errors() {
        let dir = build_repo("hermes-skills");
        let root = dir.path();

        // Don't create the source skill directory
        let manifest = make_manifest(vec![Source {
            name: "hermes-skills".to_string(),
            skills: vec![SkillEntry::unmodified("missing-skill")],
        }]);

        let err = sync_unmodified(root, &manifest).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("does not exist"),
            "error should mention missing dir: {msg}"
        );
        assert!(
            msg.contains("missing-skill"),
            "error should mention skill path: {msg}"
        );
    }

    #[test]
    fn test_empty_source_skill_dir() {
        let dir = build_repo("hermes-skills");
        let root = dir.path();

        // Create an empty source skill directory
        fs::create_dir_all(root.join("sources/hermes-skills/empty-skill")).unwrap();

        let manifest = make_manifest(vec![Source {
            name: "hermes-skills".to_string(),
            skills: vec![SkillEntry::unmodified("empty-skill")],
        }]);

        let result = sync_unmodified(root, &manifest).unwrap();

        // An empty dir should sync (creating empty dest dir)
        assert_eq!(result.synced.len(), 1);
        assert!(exists(root, "skills/hermes-skills/empty-skill"));

        // Re-running should report unchanged
        let result2 = sync_unmodified(root, &manifest).unwrap();
        assert_eq!(result2.unchanged.len(), 1);
    }

    #[test]
    fn test_dir_contents_equal_nonexistent_dst() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("file.txt"), "content").unwrap();

        // dst doesn't exist
        assert!(!dir_contents_equal(&src, &dst).unwrap());
    }

    #[test]
    fn test_dir_contents_equal_identical() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        fs::write(src.join("a.txt"), "aaa").unwrap();
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/b.txt"), "bbb").unwrap();
        fs::write(dst.join("a.txt"), "aaa").unwrap();
        fs::create_dir_all(dst.join("sub")).unwrap();
        fs::write(dst.join("sub/b.txt"), "bbb").unwrap();

        assert!(dir_contents_equal(&src, &dst).unwrap());
    }

    #[test]
    fn test_dir_contents_equal_different_content() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        fs::write(src.join("a.txt"), "aaa").unwrap();
        fs::write(dst.join("a.txt"), "different").unwrap();

        assert!(!dir_contents_equal(&src, &dst).unwrap());
    }

    // Keep PathBuf import used (for potential future tests)
    #[test]
    fn test_pathbuf_unused() {
        let _: PathBuf = PathBuf::from("x");
    }
}
