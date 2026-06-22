//! Import and lint commands for manifest management.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::manifest::{Manifest, SkillEntry, Source};

/// Find all skill directories under a given path.
/// A skill directory is one that contains a `SKILL.md` file.
///
/// Returns a list of (relative_path, name, description) tuples where
/// relative_path is relative to the search path.
fn find_skills(search_path: &Path) -> Result<Vec<FoundSkill>> {
    let mut skills = Vec::new();

    for entry in walkdir::WalkDir::new(search_path)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_dir() {
            continue;
        }

        let skill_md = entry.path().join("SKILL.md");
        if skill_md.exists() {
            let relative = entry
                .path()
                .strip_prefix(search_path)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .replace('\\', "/");

            let (name, description) = parse_skill_frontmatter(&skill_md).unwrap_or_else(|e| {
                eprintln!("warning: could not parse {}: {}", skill_md.display(), e);
                (relative.clone(), String::new())
            });

            skills.push(FoundSkill {
                path: relative,
                name,
                description,
            });
        }
    }

    Ok(skills)
}

/// A skill discovered during import scanning.
#[derive(Debug, Clone)]
pub struct FoundSkill {
    pub path: String,
    pub name: String,
    pub description: String,
}

/// Parse the YAML frontmatter from a SKILL.md file.
/// Returns (name, description).
fn parse_skill_frontmatter(skill_md_path: &Path) -> Result<(String, String)> {
    let content = fs::read_to_string(skill_md_path)
        .with_context(|| format!("read {}", skill_md_path.display()))?;

    // Frontmatter is delimited by --- at the start of the file
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        // No frontmatter — use the directory name
        let name = skill_md_path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        return Ok((name, String::new()));
    }

    // Find the closing ---
    let after_first = &trimmed[3..];
    let end = after_first.find("\n---").with_context(|| {
        format!(
            "no closing --- in frontmatter of {}",
            skill_md_path.display()
        )
    })?;

    let frontmatter = &after_first[..end];

    // Parse name and description from the frontmatter YAML
    let name = extract_yaml_field(frontmatter, "name").unwrap_or_else(|| {
        skill_md_path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    });

    let description = extract_yaml_field(frontmatter, "description").unwrap_or_default();

    Ok((name, description))
}

/// Extract a simple top-level YAML field value (handles quoted and unquoted strings).
fn extract_yaml_field(yaml: &str, field: &str) -> Option<String> {
    for line in yaml.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(&format!("{field}:")) {
            let value = rest.trim();
            // Strip surrounding quotes
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            return Some(value.to_string());
        }
    }
    None
}

/// Find the git root of a submodule by searching for `.git` from the given path upward.
/// Returns (git_root, relative_path_from_git_root).
fn find_git_root(path: &Path) -> Result<(PathBuf, String)> {
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    let mut current = abs_path.as_path();
    loop {
        if current.join(".git").exists() {
            let relative = abs_path
                .strip_prefix(current)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            return Ok((current.to_path_buf(), relative));
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => bail!(
                "path '{}' is not inside a git repository (no .git found)",
                abs_path.display()
            ),
        }
    }
}

/// Run the `import` command: scan a path for skills, let the user select which to add.
pub fn run_import(repo_root: &Path, search_path: &str) -> Result<()> {
    // Resolve the search path relative to repo root if it's a relative path
    let abs_search = if Path::new(search_path).is_absolute() {
        PathBuf::from(search_path)
    } else {
        repo_root.join(search_path)
    };

    if !abs_search.exists() {
        bail!("path '{}' does not exist", abs_search.display());
    }

    // Find the git root of the submodule
    let (git_root, relative_from_git) = find_git_root(&abs_search)?;

    // Source name = the directory name of the git root
    let source_name = git_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .context("could not determine source name from git root")?;

    // base_path = the path from git root to the search path
    // e.g., if git_root is sources/hermes-agent and search_path is sources/hermes-agent/skills
    // then base_path = "skills"
    let base_path = if relative_from_git.is_empty() {
        None
    } else {
        Some(relative_from_git)
    };

    // Find skills under the search path
    let found_skills = find_skills(&abs_search)?;

    if found_skills.is_empty() {
        println!("No skills found under {}", abs_search.display());
        return Ok(());
    }

    // Display the skills and let user select
    println!(
        "Found {} skill(s) in source '{}' (base_path: {}):\n",
        found_skills.len(),
        source_name,
        base_path.as_deref().unwrap_or("(root)")
    );

    let selected = toggle_select(&found_skills)?;

    if selected.is_empty() {
        println!("No skills selected. Exiting.");
        return Ok(());
    }

    // Load existing manifest or create new one
    let manifest_path = repo_root.join("skills-manifest.yaml");
    let mut manifest = if manifest_path.exists() {
        Manifest::from_file(&manifest_path)?
    } else {
        Manifest {
            sources: Vec::new(),
        }
    };

    // Check if a source with this name + base_path already exists
    let existing = manifest
        .sources
        .iter_mut()
        .find(|s| s.name == source_name && s.base_path.as_deref() == base_path.as_deref());

    let added_skills: Vec<&FoundSkill>;
    if let Some(source) = existing {
        // Add skills to existing source, skipping duplicates
        let mut count = 0;
        for skill in &selected {
            if !source.skills.iter().any(|s| s.path == skill.path) {
                source.skills.push(SkillEntry {
                    path: skill.path.clone(),
                    modified: false,
                    mod_tags: None,
                });
                count += 1;
            } else {
                eprintln!(
                    "warning: skill '{}' already in manifest, skipping",
                    skill.path
                );
            }
        }
        added_skills = selected.iter().take(count).collect();
    } else {
        // Create new source entry
        let skills: Vec<SkillEntry> = selected
            .iter()
            .map(|s| SkillEntry {
                path: s.path.clone(),
                modified: false,
                mod_tags: None,
            })
            .collect();
        added_skills = selected.iter().collect();
        manifest.sources.push(Source {
            name: source_name.clone(),
            ref_: None,
            base_path: base_path.clone(),
            skills,
        });
    }

    // Save manifest
    manifest.save(repo_root)?;

    // Print summary
    println!("\nAdded {} skill(s) to manifest:", added_skills.len());
    for skill in &added_skills {
        println!("  {} — {}", skill.path, skill.name);
    }
    println!(
        "\nSource: {} (base_path: {})",
        source_name,
        base_path.as_deref().unwrap_or("(root)")
    );

    Ok(())
}

/// Display a toggle list and let the user select items.
/// Uses crossterm raw mode for character-at-a-time key handling.
fn toggle_select(skills: &[FoundSkill]) -> Result<Vec<FoundSkill>> {
    use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use crossterm::terminal;

    let mut selected: Vec<bool> = vec![false; skills.len()];
    let mut cursor = 0usize;

    // Enter raw mode
    terminal::enable_raw_mode().context("failed to enable raw mode")?;

    // Ensure we disable raw mode no matter what
    let result = (|| -> Result<Vec<FoundSkill>> {
        loop {
            print_toggle_list(skills, &selected, cursor);

            if !event::poll(std::time::Duration::from_millis(500))? {
                continue;
            }

            let event = event::read()?;
            let Event::Key(KeyEvent {
                code,
                kind,
                modifiers,
                ..
            }) = event
            else {
                continue;
            };

            // Only handle key press events (not release/repeat on some terminals)
            if kind != KeyEventKind::Press && kind != KeyEventKind::Repeat {
                continue;
            }

            // Ctrl+C exits immediately
            if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
                return Ok(Vec::new());
            }

            match code {
                KeyCode::Enter => break,
                KeyCode::Char('q') | KeyCode::Char('Q') => {
                    return Ok(Vec::new());
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    // Toggle: select all if not all selected, unselect all if all selected
                    let all_selected = selected.iter().all(|&s| s);
                    if all_selected {
                        selected.fill(false);
                    } else {
                        selected.fill(true);
                    }
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    if cursor + 1 < skills.len() {
                        cursor += 1;
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    cursor = cursor.saturating_sub(1);
                }
                KeyCode::Char(' ') => {
                    selected[cursor] = !selected[cursor];
                    if cursor + 1 < skills.len() {
                        cursor += 1;
                    }
                }
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    if let Some(n) = c.to_digit(10) {
                        let idx = n as usize;
                        if idx > 0 && idx <= skills.len() {
                            selected[idx - 1] = !selected[idx - 1];
                        }
                    }
                }
                _ => {}
            }
        }

        let result: Vec<FoundSkill> = skills
            .iter()
            .zip(&selected)
            .filter(|(_, &sel)| sel)
            .map(|(s, _)| s.clone())
            .collect();

        Ok(result)
    })();

    // Always disable raw mode and print newline
    let _ = terminal::disable_raw_mode();
    println!();

    result
}

fn print_toggle_list(skills: &[FoundSkill], selected: &[bool], cursor: usize) {
    use crossterm::cursor;
    use crossterm::execute;
    use crossterm::terminal;

    // Clear screen and move cursor to top-left
    let _ = execute!(
        io::stdout(),
        terminal::Clear(terminal::ClearType::All),
        cursor::MoveTo(0, 0)
    );

    let lines: Vec<String> = std::iter::once(
        "Select skills to import (Space to toggle, 'a' to toggle all, Enter to confirm, Ctrl+C to cancel)\r"
        .to_string(),
    )
    .chain(std::iter::once(String::new()))
    .chain(skills.iter().enumerate().flat_map(|(i, skill)| {
        let marker = if selected[i] { "[x]" } else { "[ ]" };
        let cursor_marker = if i == cursor { ">" } else { " " };
        let mut lines = Vec::new();

        let name_display = if skill.name.is_empty() {
            String::new()
        } else {
            format!(" — {}", skill.name)
        };

        lines.push(format!(
            "{} {} {:<50}{}\r",
            cursor_marker, marker, skill.path, name_display
        ));

        if !skill.description.is_empty() {
            lines.push(format!("      {}\r", skill.description));
        }

        lines
    }))
    .collect();

    // Write all lines at once
    let output = lines.join("\n");
    print!("{}", output);
    let _ = io::stdout().flush();
}

/// Run the `lint` command: validate the manifest and report any issues.
pub fn run_lint(repo_root: &Path) -> Result<()> {
    let manifest_path = repo_root.join("skills-manifest.yaml");

    if !manifest_path.exists() {
        bail!("no skills-manifest.yaml found at {}", repo_root.display());
    }

    // Try to load and validate the manifest
    match Manifest::load(repo_root) {
        Ok(manifest) => {
            let total_skills: usize = manifest.sources.iter().map(|s| s.skills.len()).sum();
            let modified_count: usize = manifest
                .sources
                .iter()
                .map(|s| s.skills.iter().filter(|sk| sk.modified).count())
                .sum();

            println!("✓ Manifest is valid");
            println!("  Sources: {}", manifest.sources.len());
            println!("  Total skills: {}", total_skills);
            println!("  Modified skills: {}", modified_count);

            // Check for potential issues beyond validation
            let mut warnings = 0;
            for source in &manifest.sources {
                // Check if the source submodule directory exists
                let submodule_path = repo_root.join("sources").join(&source.name);
                if !submodule_path.exists() {
                    println!(
                        "  ⚠ source '{}' submodule directory not found at {}",
                        source.name,
                        submodule_path.display()
                    );
                    warnings += 1;
                }

                // Check if skills exist in the submodule
                for skill in &source.skills {
                    let skill_path = match &source.base_path {
                        Some(bp) if !bp.is_empty() => submodule_path.join(bp).join(&skill.path),
                        _ => submodule_path.join(&skill.path),
                    };
                    if !skill_path.exists() && submodule_path.exists() {
                        println!(
                            "  ⚠ skill '{}' not found at {}",
                            skill.path,
                            skill_path.display()
                        );
                        warnings += 1;
                    }
                }
            }

            if warnings > 0 {
                println!("\n{} warning(s) found.", warnings);
            } else {
                println!("\nNo warnings. All good!");
            }

            Ok(())
        }
        Err(e) => {
            println!("✗ Manifest has errors:\n");
            println!("  {}", e);
            // Exit with error
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_skill_dir(root: &Path, rel: &str, name: &str, description: &str) {
        let skill_dir = root.join(rel);
        fs::create_dir_all(&skill_dir).unwrap();
        let skill_md = format!(
            "---\nname: {name}\ndescription: \"{description}\"\n---\n# {name}\nSkill content\n"
        );
        fs::write(skill_dir.join("SKILL.md"), skill_md).unwrap();
    }

    #[test]
    fn test_find_skills_finds_skill_directories() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        create_skill_dir(root, "devops/kanban", "kanban", "Kanban orchestration");
        create_skill_dir(root, "mlops/inference", "inference", "LLM inference");
        // Non-skill directory (no SKILL.md)
        fs::create_dir_all(root.join("empty-dir")).unwrap();

        let skills = find_skills(root).unwrap();
        assert_eq!(skills.len(), 2);

        // Sort by path for deterministic comparison
        let mut skills_sorted = skills.clone();
        skills_sorted.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(skills_sorted[0].path, "devops/kanban");
        assert_eq!(skills_sorted[0].name, "kanban");
        assert_eq!(skills_sorted[1].path, "mlops/inference");
        assert_eq!(skills_sorted[1].name, "inference");
    }

    #[test]
    fn test_find_skills_empty_dir() {
        let dir = TempDir::new().unwrap();
        let skills = find_skills(dir.path()).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn test_parse_skill_frontmatter() {
        let dir = TempDir::new().unwrap();
        create_skill_dir(dir.path(), "my-skill", "my-skill", "A test skill");

        let skill_md = dir.path().join("my-skill").join("SKILL.md");
        let (name, desc) = parse_skill_frontmatter(&skill_md).unwrap();
        assert_eq!(name, "my-skill");
        assert_eq!(desc, "A test skill");
    }

    #[test]
    fn test_parse_skill_frontmatter_no_frontmatter() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("plain-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "# Plain skill\nNo frontmatter\n",
        )
        .unwrap();

        let (name, _desc) = parse_skill_frontmatter(&skill_dir.join("SKILL.md")).unwrap();
        assert_eq!(name, "plain-skill");
    }

    #[test]
    fn test_parse_skill_frontmatter_single_quotes() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("quoted");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: 'quoted-skill'\ndescription: 'Single quoted desc'\n---\n# Content\n",
        )
        .unwrap();

        let (name, desc) = parse_skill_frontmatter(&skill_dir.join("SKILL.md")).unwrap();
        assert_eq!(name, "quoted-skill");
        assert_eq!(desc, "Single quoted desc");
    }

    #[test]
    fn test_extract_yaml_field() {
        let yaml = "name: test-skill\ndescription: \"A description\"\nother: value";
        assert_eq!(
            extract_yaml_field(yaml, "name"),
            Some("test-skill".to_string())
        );
        assert_eq!(
            extract_yaml_field(yaml, "description"),
            Some("A description".to_string())
        );
        assert_eq!(extract_yaml_field(yaml, "missing"), None);
    }

    #[test]
    fn test_find_git_root() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        // Create a fake git repo
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("skills/subdir")).unwrap();

        let (git_root, relative) = find_git_root(&root.join("skills/subdir")).unwrap();
        assert_eq!(git_root, root.to_path_buf());
        assert_eq!(relative, "skills/subdir");

        // From the git root itself, relative is empty
        let (git_root2, relative2) = find_git_root(root).unwrap();
        assert_eq!(git_root2, root.to_path_buf());
        assert_eq!(relative2, "");
    }

    #[test]
    fn test_find_git_root_not_in_repo() {
        let dir = TempDir::new().unwrap();
        let result = find_git_root(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_run_lint_valid_manifest() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        fs::write(
            root.join("skills-manifest.yaml"),
            r#"
sources:
  - name: test-source
    skills:
      - path: skill-a
        modified: false
"#,
        )
        .unwrap();

        // Should succeed (no validation errors)
        let result = run_lint(root);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_lint_no_manifest() {
        let dir = TempDir::new().unwrap();
        let result = run_lint(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_run_lint_missing_submodule_warns() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        fs::write(
            root.join("skills-manifest.yaml"),
            r#"
sources:
  - name: missing-source
    skills:
      - path: skill-a
        modified: false
"#,
        )
        .unwrap();

        // Should succeed but warn about missing submodule
        let result = run_lint(root);
        assert!(result.is_ok());
    }
}
