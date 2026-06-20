//! Git submodule ref resolution utilities.
//!
//! Shells out to the `git` CLI to query submodule refs. All comparisons are done
//! by resolved commit SHA, never by ref name, so that moved tags or rebased
//! branches are correctly detected.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

/// Outcome of a `git merge-file` operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// The merge completed without conflicts.
    Clean,
    /// The merge completed with the given number of conflicting hunks.
    Conflicts(usize),
}

/// Get the current HEAD commit SHA of a submodule.
///
/// Runs `git -C <submodule_path> rev-parse HEAD`.
/// Returns an error if the path doesn't exist, isn't a git repo, or git fails.
pub fn submodule_ref(submodule_path: &Path) -> Result<String> {
    if !submodule_path.exists() {
        bail!(
            "submodule path does not exist (not checked out?): {}",
            submodule_path.display()
        );
    }
    run_git_rev_parse(submodule_path, "HEAD").with_context(|| {
        format!(
            "failed to get HEAD of submodule at {}",
            submodule_path.display()
        )
    })
}

/// Resolve a ref name (tag, branch, or SHA) to a full commit SHA.
///
/// Runs `git -C <submodule_path> rev-parse <ref_name>`.
/// Returns a clear error identifying the source and ref if the ref doesn't exist.
#[allow(dead_code)] // used by sync logic in a later issue
pub fn resolve_ref(submodule_path: &Path, ref_name: &str) -> Result<String> {
    if !submodule_path.exists() {
        bail!(
            "submodule path does not exist (not checked out?): {}",
            submodule_path.display()
        );
    }
    run_git_rev_parse(submodule_path, ref_name).with_context(|| {
        format!(
            "failed to resolve ref '{}' in submodule at {} — ref may not exist (tag deleted, branch removed, or typo)",
            ref_name,
            submodule_path.display()
        )
    })
}

/// Convenience wrapper: map a source name to `sources/<source_name>` under
/// `repo_root` and return the submodule's current HEAD SHA.
pub fn get_submodule_head_sha(repo_root: &Path, source_name: &str) -> Result<String> {
    let submodule_path: PathBuf = repo_root.join("sources").join(source_name);
    submodule_ref(&submodule_path).with_context(|| {
        format!(
            "failed to get HEAD SHA for source '{}' (path: {})",
            source_name,
            submodule_path.display()
        )
    })
}

/// Run `git -C <path> rev-parse <ref>` and return the trimmed SHA.
fn run_git_rev_parse(path: &Path, ref_name: &str) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-parse")
        .arg(ref_name)
        .output()
        .with_context(|| format!("failed to spawn git process for path {}", path.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = stderr.trim();
        return Err(anyhow!(
            "git rev-parse {} failed in {}: {}",
            ref_name,
            path.display(),
            if trimmed.is_empty() {
                "(no error message from git)"
            } else {
                trimmed
            }
        ));
    }

    let stdout = String::from_utf8(output.stdout).context("git rev-parse output was not UTF-8")?;
    Ok(stdout.trim().to_string())
}

/// Extract a file's contents from git history at a specific ref.
///
/// Runs `git -C <submodule_path> show <ref_name>:<file_path>`.
/// Returns `Ok(Some(content))` if the file exists at that ref.
/// Returns `Ok(None)` if the file doesn't exist at that ref (exit code 1 from git).
/// Returns `Err` on other git failures.
pub fn show_file_at_ref(
    submodule_path: &Path,
    ref_name: &str,
    file_path: &str,
) -> Result<Option<String>> {
    let spec = format!("{ref_name}:{file_path}");
    let output = Command::new("git")
        .arg("-C")
        .arg(submodule_path)
        .arg("show")
        .arg(&spec)
        .output()
        .with_context(|| {
            format!(
                "failed to spawn git show for {} in {}",
                spec,
                submodule_path.display()
            )
        })?;

    if output.status.success() {
        let content = String::from_utf8(output.stdout)
            .with_context(|| format!("git show {} output was not UTF-8", spec))?;
        return Ok(Some(content));
    }

    // git returns exit code 128 for various errors including nonexistent files.
    // Check stderr for "does not exist" to distinguish from real failures.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let trimmed = stderr.trim();
    if trimmed.contains("does not exist") || trimmed.contains("exists on disk, but not in") {
        return Ok(None);
    }

    Err(anyhow!(
        "git show {} failed in {}: {}",
        spec,
        submodule_path.display(),
        if trimmed.is_empty() {
            "(no error message from git)"
        } else {
            trimmed
        }
    ))
}

/// Run `git merge-file` to perform a 3-way merge.
///
/// Runs `git merge-file --marker-size 7 <ours> <base> <theirs>`.
/// `git merge-file` writes the merged result INTO the `ours` file in-place.
///
/// - Exit code 0: clean merge → returns `MergeOutcome::Clean`
/// - Exit code > 0: number of conflicting hunks → returns `MergeOutcome::Conflicts(n)`
/// - Exit code < 0: error
pub fn merge_file(ours: &Path, base: &Path, theirs: &Path) -> Result<MergeOutcome> {
    let output = Command::new("git")
        .arg("merge-file")
        .arg("--marker-size")
        .arg("7")
        .arg(ours)
        .arg(base)
        .arg(theirs)
        .output()
        .with_context(|| {
            format!(
                "failed to spawn git merge-file for ours={}, base={}, theirs={}",
                ours.display(),
                base.display(),
                theirs.display()
            )
        })?;

    let code = output.status.code().unwrap_or(-1);
    if code < 0 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "git merge-file failed (code {}): {}",
            code,
            stderr.trim()
        ));
    }

    if code == 0 {
        Ok(MergeOutcome::Clean)
    } else {
        Ok(MergeOutcome::Conflicts(code as usize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Helper: create a real git repo in a temp dir with one commit.
    fn make_temp_repo() -> TempDir {
        let dir = TempDir::new().expect("failed to create temp dir");
        let path = dir.path();

        // git init
        let status = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["init"])
            .status()
            .expect("git init failed");
        assert!(status.success(), "git init did not succeed");

        // set local identity (in case global isn't configured in test env)
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.email", "test@example.com"])
            .status()
            .expect("git config email failed");
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.name", "Test"])
            .status()
            .expect("git config name failed");

        // commit a file so HEAD exists
        fs::write(path.join("README.md"), "hello\n").expect("write file failed");
        let status = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["add", "-A"])
            .status()
            .expect("git add failed");
        assert!(status.success());

        // use -c to allow empty branch creation in newer git
        let status = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["commit", "-m", "initial"])
            .status()
            .expect("git commit failed");
        assert!(status.success(), "git commit did not succeed");

        dir
    }

    #[test]
    fn test_submodule_ref_nonexistent_path_errors() {
        let dir = TempDir::new().unwrap();
        let bogus = dir.path().join("does-not-exist");
        let err = submodule_ref(&bogus).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("does not exist"),
            "error should mention path doesn't exist: got: {msg}"
        );
    }

    #[test]
    fn test_resolve_ref_nonexistent_ref_errors() {
        let dir = make_temp_repo();
        let err = resolve_ref(dir.path(), "refs/heads/nonexistent-branch").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("nonexistent-branch"),
            "error should mention the ref: got: {msg}"
        );
    }

    #[test]
    fn test_submodule_ref_on_real_repo() {
        let dir = make_temp_repo();
        let sha = submodule_ref(dir.path()).expect("submodule_ref should succeed on real repo");
        // A commit SHA is 40 hex chars
        assert!(
            sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
            "expected a 40-char hex SHA, got: {sha}"
        );
        assert!(!sha.is_empty());
    }

    #[test]
    fn test_resolve_ref_head_on_real_repo() {
        let dir = make_temp_repo();
        let head_sha = submodule_ref(dir.path()).unwrap();
        let resolved = resolve_ref(dir.path(), "HEAD").unwrap();
        assert_eq!(head_sha, resolved, "HEAD and rev-parse HEAD should match");
    }

    #[test]
    fn test_get_submodule_head_sha_nonexistent_source() {
        let dir = TempDir::new().unwrap();
        let err = get_submodule_head_sha(dir.path(), "no-such-source").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("no-such-source"),
            "error should mention source name: got: {msg}"
        );
    }

    #[test]
    fn test_get_submodule_head_sha_real_repo() {
        // Simulate a repo_root with sources/<name> being a git repo
        let root = TempDir::new().unwrap();
        let sources_dir = root.path().join("sources");
        fs::create_dir_all(&sources_dir).unwrap();
        // init a git repo at sources/my-source
        let source_path = sources_dir.join("my-source");
        fs::create_dir(&source_path).unwrap();

        Command::new("git")
            .arg("-C")
            .arg(&source_path)
            .args(["init"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&source_path)
            .args(["config", "user.email", "t@example.com"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&source_path)
            .args(["config", "user.name", "T"])
            .status()
            .unwrap();
        fs::write(source_path.join("f.txt"), "x\n").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&source_path)
            .args(["add", "-A"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&source_path)
            .args(["commit", "-m", "x"])
            .status()
            .unwrap();

        let sha = get_submodule_head_sha(root.path(), "my-source").unwrap();
        assert_eq!(sha.len(), 40);
    }

    #[test]
    fn test_show_file_at_ref_existing_file() {
        let dir = make_temp_repo();
        let content = show_file_at_ref(dir.path(), "HEAD", "README.md").unwrap();
        assert_eq!(content, Some("hello\n".to_string()));
    }

    #[test]
    fn test_show_file_at_ref_nonexistent_file() {
        let dir = make_temp_repo();
        let content = show_file_at_ref(dir.path(), "HEAD", "no-such-file.md").unwrap();
        assert_eq!(content, None);
    }

    #[test]
    fn test_merge_file_clean_merge() {
        let dir = TempDir::new().unwrap();
        // base has 3 lines, ours changes line 1, theirs changes line 3
        let base = dir.path().join("base.txt");
        let ours = dir.path().join("ours.txt");
        let theirs = dir.path().join("theirs.txt");

        fs::write(&base, "line1\nline2\nline3\n").unwrap();
        fs::write(&ours, "OURS\nline2\nline3\n").unwrap();
        fs::write(&theirs, "line1\nline2\nTHEIRS\n").unwrap();

        let outcome = merge_file(&ours, &base, &theirs).unwrap();
        assert_eq!(outcome, MergeOutcome::Clean);

        let result = fs::read_to_string(&ours).unwrap();
        assert!(result.contains("OURS"));
        assert!(result.contains("THEIRS"));
        assert!(!result.contains("<<<<<<<"));
    }

    #[test]
    fn test_merge_file_conflict() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().join("base.txt");
        let ours = dir.path().join("ours.txt");
        let theirs = dir.path().join("theirs.txt");

        fs::write(&base, "line1\nline2\nline3\n").unwrap();
        fs::write(&ours, "line1\nOURS\nline3\n").unwrap();
        fs::write(&theirs, "line1\nTHEIRS\nline3\n").unwrap();

        let outcome = merge_file(&ours, &base, &theirs).unwrap();
        match outcome {
            MergeOutcome::Conflicts(n) => assert!(n >= 1, "expected at least 1 conflict"),
            MergeOutcome::Clean => panic!("expected conflict, got clean merge"),
        }

        let result = fs::read_to_string(&ours).unwrap();
        assert!(result.contains("<<<<<<<"), "expected conflict markers");
    }
}
