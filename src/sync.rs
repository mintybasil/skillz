//! Sync unmodified skills from git submodules into the bundle directory.
//!
//! For each source in the manifest, for each skill where `modified: false`:
//! - Source: `sources/<source_name>/<base_path>/<skill_path>` (when base_path is set)
//! - Source: `sources/<source_name>/<skill_path>` (when base_path is empty/unset)
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

use crate::git;
use crate::manifest::{Manifest, SkillEntry, Source};

/// Result of syncing unmodified skills.
#[derive(Debug, Clone, Default)]
pub struct SyncResult {
    /// Skills that were copied (source_name, skill_path).
    pub synced: Vec<(String, String)>,
    /// Skills that were already up to date (source_name, skill_path).
    pub unchanged: Vec<(String, String)>,
    /// Modified skills that were merged cleanly.
    pub merged: Vec<MergeResult>,
    /// Modified skills where the submodule HEAD matches ref (no upstream change).
    pub up_to_date: Vec<(String, String)>,
    /// Modified skills that had conflicts during merge.
    pub conflicted: Vec<MergeResult>,
    /// Drift warnings for skills whose ref could not be resolved.
    /// Each tuple is (source_name, skill_path, ref).
    pub drift_warnings: Vec<(String, String, String)>,
}

impl SyncResult {
    /// Returns true if any ref was updated during sync_modified.
    pub fn any_ref_updated(&self) -> bool {
        self.merged.iter().any(|m| m.new_ref != m.old_ref)
    }
}

/// Result of merging a modified skill.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MergeResult {
    pub source_name: String,
    pub skill_path: String,
    pub old_ref: String,
    /// New ref after merge. Same as old_ref if there were conflicts
    /// (ref is only updated when all files merge cleanly).
    pub new_ref: String,
    pub files_merged: Vec<String>,
    pub files_conflicted: Vec<String>,
}

/// Build the source skill path relative to the submodule root, accounting for base_path.
///
/// When base_path is set: `<base_path>/<skill_path>`
/// When base_path is empty/unset: `<skill_path>`
fn build_src_skill_subpath(base_path: &Option<String>, skill_path: &str) -> PathBuf {
    match base_path {
        Some(bp) if !bp.is_empty() => PathBuf::from(bp).join(skill_path),
        _ => PathBuf::from(skill_path),
    }
}

/// Sync all unmodified skills from submodules into the bundle directory.
///
/// Iterates over every source in the manifest, and for each skill where
/// `modified: false`, copies the skill directory from `sources/<source>/<base_path>/<path>`
/// (or `sources/<source>/<path>` when base_path is unset) to `skills/<source>/<path>`
/// using a clear-then-copy strategy. Skills that are already byte-for-byte identical
/// are skipped to reduce git diff noise.
pub fn sync_unmodified(repo_root: &Path, manifest: &Manifest) -> Result<SyncResult> {
    let sources_root = repo_root.join("sources");
    let bundle_root = repo_root.join("skills");

    let mut result = SyncResult::default();

    for source in &manifest.sources {
        for skill in &source.skills {
            if skill.modified {
                continue;
            }

            let src_subpath = build_src_skill_subpath(&source.base_path, &skill.path);
            let src_path = sources_root.join(&source.name).join(&src_subpath);
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

/// Sync modified skills from submodules into the bundle directory using 3-way merge.
///
/// For each source in the manifest that has modified skills:
/// 1. Get the submodule's current HEAD SHA.
/// 2. Resolve source.ref to a SHA.
/// 3. If SHAs match: all modified skills in that source are up to date, skip.
/// 4. If SHAs differ: perform 3-way merge per skill using `git merge-file`.
///
/// `ref` is only updated in the manifest when ALL files in ALL modified skills
/// in a source merge cleanly. If any files conflict, `ref` is left unchanged so
/// the operator can resolve and re-run.
pub fn sync_modified(repo_root: &Path, manifest: &mut Manifest) -> Result<SyncResult> {
    let sources_root = repo_root.join("sources");
    let bundle_root = repo_root.join("skills");
    let mut result = SyncResult::default();

    for source in &mut manifest.sources {
        // Collect modified skill indices for this source
        let modified_indices: Vec<usize> = source
            .skills
            .iter()
            .enumerate()
            .filter(|(_, s)| s.modified)
            .map(|(i, _)| i)
            .collect();

        if modified_indices.is_empty() {
            continue;
        }

        let submodule_path = sources_root.join(&source.name);

        let source_ref = source
            .ref_
            .as_ref()
            .expect("source with modified skills must have ref (validated at load time)");

        // Get current submodule HEAD sha once per source
        let current_sha = git::get_submodule_head_sha(repo_root, &source.name)?;

        // Resolve source ref to sha once per source.
        // If the ref can't be resolved, record drift warnings for all modified skills.
        let base_sha = match git::resolve_ref(&submodule_path, source_ref) {
            Ok(sha) => sha,
            Err(_) => {
                for &idx in &modified_indices {
                    let skill = &source.skills[idx];
                    result.drift_warnings.push((
                        source.name.clone(),
                        skill.path.clone(),
                        source_ref.clone(),
                    ));
                }
                continue;
            }
        };

        if current_sha == base_sha {
            // All modified skills in this source are up to date
            for &idx in &modified_indices {
                let skill = &source.skills[idx];
                result
                    .up_to_date
                    .push((source.name.clone(), skill.path.clone()));
            }
            continue;
        }

        // SHAs differ — merge all modified skills, tracking if any had conflicts
        let mut any_conflicts = false;
        let mut all_clean = true;

        for &idx in &modified_indices {
            let skill = &source.skills[idx];
            let skill_path = &skill.path;
            let src_subpath = build_src_skill_subpath(&source.base_path, skill_path);
            let src_skill_dir = submodule_path.join(&src_subpath);
            let dst_skill_dir = bundle_root.join(&source.name).join(skill_path);

            let merge_result = if !dst_skill_dir.exists() {
                // First-time modified skill: not in bundle yet.
                // Copy from submodule and set ref to current HEAD.
                copy_skill_dir(&src_skill_dir, &dst_skill_dir)?;
                MergeResult {
                    source_name: source.name.clone(),
                    skill_path: skill_path.clone(),
                    old_ref: source_ref.clone(),
                    new_ref: current_sha.clone(),
                    files_merged: vec![],
                    files_conflicted: vec![],
                }
            } else {
                merge_modified_skill(
                    &submodule_path,
                    &dst_skill_dir,
                    &source.name,
                    skill_path,
                    source_ref,
                    &current_sha,
                    &source.base_path,
                )?
            };

            if merge_result.files_conflicted.is_empty() {
                result.merged.push(merge_result);
            } else {
                result.conflicted.push(merge_result);
                any_conflicts = true;
                all_clean = false;
            }
        }

        // Update source ref after clean merges (no conflicts in any skill)
        if all_clean && !any_conflicts {
            source.ref_ = Some(current_sha);
        }
    }

    Ok(result)
}

/// Perform a 3-way merge for a single modified skill.
///
/// Enumerates files from BOTH the bundle (ours) and the submodule working tree
/// (theirs) to catch all cases. For each file:
/// - Get base content from git history at `source_ref`.
/// - If base is None (file didn't exist at ref):
///   - If file exists in theirs but not ours: copy theirs into bundle (added upstream)
///   - If file exists in ours but not theirs: keep as-is (only in modified version)
/// - If base is Some: run `git merge-file` on (ours, base, theirs).
fn merge_modified_skill(
    submodule_path: &Path,
    dst_skill_dir: &Path,
    source_name: &str,
    skill_path: &str,
    source_ref: &str,
    current_sha: &str,
    base_path: &Option<String>,
) -> Result<MergeResult> {
    let ours_files = collect_file_map(dst_skill_dir)?;
    // theirs = files in the submodule working tree at src_skill_dir
    let src_subpath = build_src_skill_subpath(base_path, skill_path);
    let src_skill_dir = submodule_path.join(&src_subpath);
    let theirs_files = collect_file_map(&src_skill_dir)?;

    // Union of all relative file paths from both ours and theirs
    let mut all_files: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    all_files.extend(ours_files.keys().cloned());
    all_files.extend(theirs_files.keys().cloned());

    let mut files_merged = Vec::new();
    let mut files_conflicted = Vec::new();
    let mut has_conflicts = false;

    // The path within the submodule for git show: <base_path>/<skill_path>/<rel>
    let git_file_prefix = match base_path {
        Some(bp) if !bp.is_empty() => format!("{bp}/{skill_path}/"),
        _ => format!("{skill_path}/"),
    };

    for rel_path in &all_files {
        let rel_str = rel_path.to_string_lossy().replace('\\', "/");
        let in_ours = ours_files.contains_key(rel_path);
        let in_theirs = theirs_files.contains_key(rel_path);

        // Get base content from git history at source_ref
        let full_path = format!("{git_file_prefix}{rel_str}");
        let base_content = git::show_file_at_ref(submodule_path, source_ref, &full_path)?;

        match (base_content, in_ours, in_theirs) {
            (None, false, true) => {
                // File added upstream, not in ours → copy theirs into bundle
                let dst_file = dst_skill_dir.join(rel_path);
                if let Some(parent) = dst_file.parent() {
                    fs::create_dir_all(parent).with_context(|| {
                        format!("failed to create parent dirs for: {}", dst_file.display())
                    })?;
                }
                let content = &theirs_files[rel_path];
                fs::write(&dst_file, content).with_context(|| {
                    format!(
                        "failed to write added-upstream file: {}",
                        dst_file.display()
                    )
                })?;
                files_merged.push(rel_str);
            }
            (None, true, false) => {
                // File only in modified version → keep as-is
                // No action needed
            }
            (None, true, true) => {
                // File exists in both ours and theirs but not at source_ref
                // (both added independently). Treat as a merge with empty base.
                let dst_file = dst_skill_dir.join(rel_path);
                run_merge_file(
                    &dst_file,
                    "",
                    &theirs_files[rel_path],
                    &rel_str,
                    &mut files_merged,
                    &mut files_conflicted,
                    &mut has_conflicts,
                )?;
            }
            (Some(base), true, true) => {
                // Standard 3-way merge
                let dst_file = dst_skill_dir.join(rel_path);
                let theirs_content = &theirs_files[rel_path];
                run_merge_file(
                    &dst_file,
                    &base,
                    theirs_content,
                    &rel_str,
                    &mut files_merged,
                    &mut files_conflicted,
                    &mut has_conflicts,
                )?;
            }
            (Some(_base), false, true) => {
                // File exists at source_ref and in theirs but was deleted in ours (modified version).
                // "File deleted in modified version: left deleted (not re-added from upstream)."
                // So we do nothing — the file stays absent from the bundle.
            }
            (Some(_base), true, false) => {
                // File exists at source_ref and in ours but was deleted upstream.
                // "File deleted upstream: keep ours as-is, warn."
                eprintln!(
                    "warning: file '{}' was deleted upstream in source '{}' — keeping modified version",
                    rel_str, source_name
                );
            }
            (Some(_), false, false) => {
                // File existed at source_ref but is deleted in both ours and theirs.
                // Nothing to do.
            }
            (None, false, false) => {
                // File doesn't exist anywhere — shouldn't happen since we only
                // iterate files from ours or theirs, but handle gracefully.
            }
        }
    }

    let new_ref = if has_conflicts {
        // Don't update ref if there were conflicts
        source_ref.to_string()
    } else {
        current_sha.to_string()
    };

    Ok(MergeResult {
        source_name: source_name.to_string(),
        skill_path: skill_path.to_string(),
        old_ref: source_ref.to_string(),
        new_ref,
        files_merged,
        files_conflicted,
    })
}

/// Helper: write base and theirs to temp files, run git merge-file, and record results.
fn run_merge_file(
    ours_path: &Path,
    base_content: &str,
    theirs_content: &[u8],
    rel_str: &str,
    files_merged: &mut Vec<String>,
    files_conflicted: &mut Vec<String>,
    has_conflicts: &mut bool,
) -> Result<()> {
    use std::io::Write;

    let tmp = tempfile::tempdir().context("failed to create temp dir for merge")?;
    let base_tmp = tmp.path().join("base");
    let theirs_tmp = tmp.path().join("theirs");

    let mut base_f = fs::File::create(&base_tmp)
        .with_context(|| format!("failed to create temp base file: {}", base_tmp.display()))?;
    base_f
        .write_all(base_content.as_bytes())
        .with_context(|| "failed to write base content to temp file")?;

    let mut theirs_f = fs::File::create(&theirs_tmp).with_context(|| {
        format!(
            "failed to create temp theirs file: {}",
            theirs_tmp.display()
        )
    })?;
    theirs_f
        .write_all(theirs_content)
        .with_context(|| "failed to write theirs content to temp file")?;

    let outcome = git::merge_file(ours_path, &base_tmp, &theirs_tmp)?;

    match outcome {
        git::MergeOutcome::Clean => {
            files_merged.push(rel_str.to_string());
        }
        git::MergeOutcome::Conflicts(_) => {
            files_conflicted.push(rel_str.to_string());
            *has_conflicts = true;
        }
    }

    Ok(())
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
    /// Create a new source with the given name, ref, base_path, and no skills.
    pub fn new(name: &str, ref_: Option<&str>, base_path: Option<&str>) -> Self {
        Source {
            name: name.to_string(),
            ref_: ref_.map(|s| s.to_string()),
            base_path: base_path.map(|s| s.to_string()),
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
            mod_tags: None,
        }
    }

    /// Create a modified skill entry at the given path.
    pub fn modified(path: &str) -> Self {
        SkillEntry {
            path: path.to_string(),
            modified: true,
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
            ref_: None,
            base_path: None,
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
    fn test_unmodified_skills_with_base_path_copied_to_bundle() {
        let dir = build_repo("hermes-skills");
        let root = dir.path();

        // Skill is under sources/hermes-skills/skills/kanban-orchestrator
        setup_source_skill(root, "hermes-skills", "skills/kanban-orchestrator");

        let manifest = make_manifest(vec![Source {
            name: "hermes-skills".to_string(),
            ref_: None,
            base_path: Some("skills".to_string()),
            skills: vec![SkillEntry::unmodified("kanban-orchestrator")],
        }]);

        let result = sync_unmodified(root, &manifest).unwrap();

        assert_eq!(result.synced.len(), 1);
        assert_eq!(result.synced[0].1, "kanban-orchestrator");

        // Verify files exist at the destination (base_path stripped from dst)
        assert!(exists(
            root,
            "skills/hermes-skills/kanban-orchestrator/README.md"
        ));
    }

    #[test]
    fn test_category_nesting_preserved() {
        let dir = build_repo("hermes-skills");
        let root = dir.path();

        // Skill with nested category path: devops/kanban-orchestrator
        setup_source_skill(root, "hermes-skills", "devops/kanban-orchestrator");

        let manifest = make_manifest(vec![Source {
            name: "hermes-skills".to_string(),
            ref_: None,
            base_path: None,
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
            ref_: None,
            base_path: None,
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
            ref_: None,
            base_path: None,
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
            ref_: Some("v1.0.0".to_string()),
            base_path: None,
            skills: vec![
                SkillEntry::unmodified("unmodified-skill"),
                SkillEntry::modified("modified-skill"),
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
            ref_: None,
            base_path: None,
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
                ref_: None,
                base_path: None,
                skills: vec![SkillEntry::unmodified("skill-a")],
            },
            Source {
                name: "second-source".to_string(),
                ref_: None,
                base_path: None,
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
            ref_: None,
            base_path: None,
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
            ref_: None,
            base_path: None,
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
            ref_: None,
            base_path: None,
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

    // ── sync_modified tests ──────────────────────────────────────────

    use std::process::Command as StdCommand;

    /// Helper: init a git repo at the given path with identity configured.
    fn init_git_repo(path: &Path) {
        StdCommand::new("git")
            .arg("-C")
            .arg(path)
            .args(["init"])
            .status()
            .expect("git init failed");
        StdCommand::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.email", "test@example.com"])
            .status()
            .expect("git config email failed");
        StdCommand::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.name", "Test"])
            .status()
            .expect("git config name failed");
    }

    /// Helper: commit all changes in a git repo at the given path.
    fn git_commit(path: &Path, msg: &str) {
        StdCommand::new("git")
            .arg("-C")
            .arg(path)
            .args(["add", "-A"])
            .status()
            .expect("git add failed");
        StdCommand::new("git")
            .arg("-C")
            .arg(path)
            .args(["commit", "-m", msg])
            .status()
            .expect("git commit failed");
    }

    /// Helper: get HEAD SHA of a git repo.
    fn git_head_sha(path: &Path) -> String {
        let output = StdCommand::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git rev-parse failed");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    /// Set up a repo with a source that is a real git repo with two commits.
    /// Returns (root_tempdir, base_sha, head_sha).
    /// The source repo at sources/<source>/<skill> has:
    ///   - commit 1 (base): file.txt with "line1\nline2\nline3\n"
    ///   - commit 2 (head): file.txt with "line1\nline2\nTHEIRS\n"
    fn setup_repo_with_git_history(source: &str, skill: &str) -> (TempDir, String, String) {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        // Create the repo structure
        fs::create_dir_all(root.join("sources").join(source).join(skill)).unwrap();
        fs::create_dir_all(root.join("skills")).unwrap();

        // Init git repo in sources/<source>
        let source_repo = root.join("sources").join(source);
        init_git_repo(&source_repo);

        // Commit 1 (base): create the skill file
        write_file(
            root,
            &format!("sources/{source}/{skill}/file.txt"),
            "line1\nline2\nline3\n",
        );
        git_commit(&source_repo, "initial");
        let base_sha = git_head_sha(&source_repo);

        // Commit 2 (head): modify the file upstream
        write_file(
            root,
            &format!("sources/{source}/{skill}/file.txt"),
            "line1\nline2\nTHEIRS\n",
        );
        git_commit(&source_repo, "upstream change");
        let head_sha = git_head_sha(&source_repo);

        (dir, base_sha, head_sha)
    }

    /// Build a manifest with a single source that has a modified skill.
    fn make_modified_manifest(source: &str, ref_: &str, skill: &str) -> Manifest {
        make_manifest(vec![Source {
            name: source.to_string(),
            ref_: Some(ref_.to_string()),
            base_path: None,
            skills: vec![SkillEntry::modified(skill)],
        }])
    }

    #[test]
    fn test_sync_modified_up_to_date() {
        let (dir, base_sha, head_sha) = setup_repo_with_git_history("src1", "my-skill");
        let root = dir.path();

        // ref = head_sha, so skill is up to date
        let mut manifest = make_modified_manifest("src1", &head_sha, "my-skill");

        let result = sync_modified(root, &mut manifest).unwrap();

        assert_eq!(result.up_to_date.len(), 1);
        assert_eq!(result.merged.len(), 0);
        assert_eq!(result.conflicted.len(), 0);
        // ref should NOT be updated (was already at head)
        assert_eq!(manifest.sources[0].ref_.as_deref(), Some(head_sha.as_str()));
        let _ = base_sha; // suppress unused warning
    }

    #[test]
    fn test_sync_modified_clean_merge() {
        let (dir, base_sha, head_sha) = setup_repo_with_git_history("src1", "my-skill");
        let root = dir.path();

        // Create the "ours" version in the bundle with a different change (line 1)
        write_file(
            root,
            "skills/src1/my-skill/file.txt",
            "OURS\nline2\nline3\n",
        );

        let mut manifest = make_modified_manifest("src1", &base_sha, "my-skill");

        let result = sync_modified(root, &mut manifest).unwrap();

        assert_eq!(result.merged.len(), 1);
        assert_eq!(result.conflicted.len(), 0);
        assert_eq!(result.up_to_date.len(), 0);

        // ref should be updated to head_sha
        assert_eq!(manifest.sources[0].ref_.as_deref(), Some(head_sha.as_str()));

        // The merged file should contain both OURS and THEIRS
        let merged = read_file(root, "skills/src1/my-skill/file.txt");
        assert!(merged.contains("OURS"));
        assert!(merged.contains("THEIRS"));
        assert!(!merged.contains("<<<<<<<"));
    }

    #[test]
    fn test_sync_modified_conflict() {
        let (dir, base_sha, head_sha) = setup_repo_with_git_history("src1", "my-skill");
        let root = dir.path();

        // Create "ours" with a conflicting change to the same line (line 3)
        write_file(
            root,
            "skills/src1/my-skill/file.txt",
            "line1\nline2\nOURS\n",
        );

        let mut manifest = make_modified_manifest("src1", &base_sha, "my-skill");

        let result = sync_modified(root, &mut manifest).unwrap();

        assert_eq!(result.conflicted.len(), 1);
        assert_eq!(result.merged.len(), 0);

        // ref should NOT be updated (conflicts)
        assert_eq!(manifest.sources[0].ref_.as_deref(), Some(base_sha.as_str()));

        // The file should contain conflict markers
        let merged = read_file(root, "skills/src1/my-skill/file.txt");
        assert!(merged.contains("<<<<<<<"), "expected conflict markers");
        let _ = head_sha;
    }

    #[test]
    fn test_sync_modified_file_added_upstream() {
        // Setup: base has file.txt, head adds new_file.txt
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let source = "src1";
        let skill = "my-skill";

        fs::create_dir_all(root.join("sources").join(source).join(skill)).unwrap();
        fs::create_dir_all(root.join("skills").join(source).join(skill)).unwrap();

        let source_repo = root.join("sources").join(source);
        init_git_repo(&source_repo);

        // Commit 1 (base): file.txt only
        write_file(
            root,
            &format!("sources/{source}/{skill}/file.txt"),
            "line1\nline2\nline3\n",
        );
        git_commit(&source_repo, "initial");
        let base_sha = git_head_sha(&source_repo);

        // Commit 2 (head): add new_file.txt
        write_file(
            root,
            &format!("sources/{source}/{skill}/new_file.txt"),
            "new content\n",
        );
        git_commit(&source_repo, "add new file");
        let head_sha = git_head_sha(&source_repo);

        // Create "ours" version — only file.txt (modified), no new_file.txt
        write_file(
            root,
            "skills/src1/my-skill/file.txt",
            "OURS\nline2\nline3\n",
        );

        let mut manifest = make_modified_manifest("src1", &base_sha, "my-skill");

        let result = sync_modified(root, &mut manifest).unwrap();

        assert_eq!(result.merged.len(), 1);
        assert_eq!(result.conflicted.len(), 0);

        // The new file should have been copied into the bundle
        assert!(exists(root, "skills/src1/my-skill/new_file.txt"));
        assert_eq!(
            read_file(root, "skills/src1/my-skill/new_file.txt"),
            "new content\n"
        );

        // ref should be updated
        assert_eq!(manifest.sources[0].ref_.as_deref(), Some(head_sha.as_str()));
    }

    #[test]
    fn test_sync_modified_file_only_in_modified() {
        let (dir, base_sha, head_sha) = setup_repo_with_git_history("src1", "my-skill");
        let root = dir.path();

        // Create "ours" with an extra file that's not in upstream
        write_file(
            root,
            "skills/src1/my-skill/file.txt",
            "OURS\nline2\nline3\n",
        );
        write_file(root, "skills/src1/my-skill/extra.txt", "extra content\n");

        let mut manifest = make_modified_manifest("src1", &base_sha, "my-skill");

        let result = sync_modified(root, &mut manifest).unwrap();

        assert_eq!(result.merged.len(), 1);
        assert_eq!(result.conflicted.len(), 0);

        // The extra file should still be there
        assert!(exists(root, "skills/src1/my-skill/extra.txt"));
        assert_eq!(
            read_file(root, "skills/src1/my-skill/extra.txt"),
            "extra content\n"
        );

        // ref should be updated
        assert_eq!(manifest.sources[0].ref_.as_deref(), Some(head_sha.as_str()));
    }

    #[test]
    fn test_sync_modified_first_time_not_in_bundle() {
        let (dir, base_sha, head_sha) = setup_repo_with_git_history("src1", "my-skill");
        let root = dir.path();

        // Don't create the bundle directory — simulate first-time modified skill
        let mut manifest = make_modified_manifest("src1", &base_sha, "my-skill");

        let result = sync_modified(root, &mut manifest).unwrap();

        // Should be treated as merged (copied from submodule)
        assert_eq!(result.merged.len(), 1);
        assert_eq!(result.conflicted.len(), 0);

        // The file should exist in the bundle
        assert!(exists(root, "skills/src1/my-skill/file.txt"));
        assert_eq!(
            read_file(root, "skills/src1/my-skill/file.txt"),
            "line1\nline2\nTHEIRS\n"
        );

        // ref should be set to current HEAD
        assert_eq!(manifest.sources[0].ref_.as_deref(), Some(head_sha.as_str()));
    }

    #[test]
    fn test_sync_modified_multiple_skills_clean_merge() {
        // Test that source.ref is updated once for all modified skills in a source
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let source = "src1";

        fs::create_dir_all(root.join("sources").join(source).join("skill-a")).unwrap();
        fs::create_dir_all(root.join("sources").join(source).join("skill-b")).unwrap();
        fs::create_dir_all(root.join("skills").join(source).join("skill-a")).unwrap();
        fs::create_dir_all(root.join("skills").join(source).join("skill-b")).unwrap();

        let source_repo = root.join("sources").join(source);
        init_git_repo(&source_repo);

        // Commit 1 (base): create both skill files
        write_file(
            root,
            &format!("sources/{source}/skill-a/file.txt"),
            "line1\nline2\nline3\n",
        );
        write_file(
            root,
            &format!("sources/{source}/skill-b/file.txt"),
            "line1\nline2\nline3\n",
        );
        git_commit(&source_repo, "initial");
        let base_sha = git_head_sha(&source_repo);

        // Commit 2 (head): modify both files upstream
        write_file(
            root,
            &format!("sources/{source}/skill-a/file.txt"),
            "line1\nline2\nTHEIRS_A\n",
        );
        write_file(
            root,
            &format!("sources/{source}/skill-b/file.txt"),
            "line1\nline2\nTHEIRS_B\n",
        );
        git_commit(&source_repo, "upstream change");
        let head_sha = git_head_sha(&source_repo);

        // Create "ours" versions with non-conflicting changes
        write_file(
            root,
            "skills/src1/skill-a/file.txt",
            "OURS_A\nline2\nline3\n",
        );
        write_file(
            root,
            "skills/src1/skill-b/file.txt",
            "OURS_B\nline2\nline3\n",
        );

        let mut manifest = make_manifest(vec![Source {
            name: source.to_string(),
            ref_: Some(base_sha.clone()),
            base_path: None,
            skills: vec![
                SkillEntry::modified("skill-a"),
                SkillEntry::modified("skill-b"),
            ],
        }]);

        let result = sync_modified(root, &mut manifest).unwrap();

        assert_eq!(result.merged.len(), 2);
        assert_eq!(result.conflicted.len(), 0);

        // ref should be updated to head_sha (all skills merged cleanly)
        assert_eq!(manifest.sources[0].ref_.as_deref(), Some(head_sha.as_str()));
    }

    #[test]
    fn test_sync_modified_conflict_in_one_skill_no_ref_update() {
        // If one skill conflicts, ref should NOT be updated even if other merged cleanly
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let source = "src1";

        fs::create_dir_all(root.join("sources").join(source).join("skill-a")).unwrap();
        fs::create_dir_all(root.join("sources").join(source).join("skill-b")).unwrap();
        fs::create_dir_all(root.join("skills").join(source).join("skill-a")).unwrap();
        fs::create_dir_all(root.join("skills").join(source).join("skill-b")).unwrap();

        let source_repo = root.join("sources").join(source);
        init_git_repo(&source_repo);

        // Commit 1 (base)
        write_file(
            root,
            &format!("sources/{source}/skill-a/file.txt"),
            "line1\nline2\nline3\n",
        );
        write_file(
            root,
            &format!("sources/{source}/skill-b/file.txt"),
            "line1\nline2\nline3\n",
        );
        git_commit(&source_repo, "initial");
        let base_sha = git_head_sha(&source_repo);

        // Commit 2 (head): modify both files
        write_file(
            root,
            &format!("sources/{source}/skill-a/file.txt"),
            "line1\nline2\nTHEIRS_A\n",
        );
        write_file(
            root,
            &format!("sources/{source}/skill-b/file.txt"),
            "line1\nline2\nTHEIRS_B\n",
        );
        git_commit(&source_repo, "upstream change");
        let head_sha = git_head_sha(&source_repo);

        // skill-a: non-conflicting change (line 1)
        write_file(
            root,
            "skills/src1/skill-a/file.txt",
            "OURS_A\nline2\nline3\n",
        );
        // skill-b: conflicting change (same line as upstream)
        write_file(
            root,
            "skills/src1/skill-b/file.txt",
            "line1\nline2\nOURS_B\n",
        );

        let mut manifest = make_manifest(vec![Source {
            name: source.to_string(),
            ref_: Some(base_sha.clone()),
            base_path: None,
            skills: vec![
                SkillEntry::modified("skill-a"),
                SkillEntry::modified("skill-b"),
            ],
        }]);

        let result = sync_modified(root, &mut manifest).unwrap();

        assert_eq!(result.merged.len(), 1); // skill-a merged cleanly
        assert_eq!(result.conflicted.len(), 1); // skill-b had conflict

        // ref should NOT be updated (one skill had conflicts)
        assert_eq!(manifest.sources[0].ref_.as_deref(), Some(base_sha.as_str()));
        let _ = head_sha;
    }
}
