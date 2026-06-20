use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Known modification tags describing the nature of local modifications to a skill.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum ModTag {
    /// Modifications for Hermes agent harness compatibility (stripping incompatible
    /// tool calls, plugins, filenames, etc.).
    #[serde(rename = "hermes-compat")]
    HermesCompat,
    /// Modifications for the operator's own preferences or customizations.
    #[serde(rename = "personalization")]
    Personalization,
}

/// Top-level manifest structure, loaded from `skills-manifest.yaml`.
#[derive(Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub sources: Vec<Source>,
}

/// An external skill source, mapped to a git submodule at `sources/<name>`.
#[derive(Debug, Deserialize, Serialize)]
pub struct Source {
    pub name: String,
    pub skills: Vec<SkillEntry>,
}

/// A single skill entry within a source.
#[derive(Debug, Deserialize, Serialize)]
pub struct SkillEntry {
    /// Path within the source repo (e.g., `devops/kanban-orchestrator`).
    pub path: String,
    /// Whether this skill has local modifications.
    #[serde(default)]
    pub modified: bool,
    /// Upstream ref the modifications are based on. Required when `modified: true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    /// Tags describing the nature of modifications.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mod_tags: Option<Vec<ModTag>>,
}

impl Manifest {
    /// Load and validate a manifest from `skills-manifest.yaml` at the repo root.
    pub fn load(repo_root: &Path) -> Result<Self> {
        let manifest_path = repo_root.join("skills-manifest.yaml");
        Self::from_file(&manifest_path)
    }

    /// Load and validate a manifest from a specific file path.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read manifest file: {}", path.display()))?;
        Self::from_str(&content)
    }

    /// Parse and validate a manifest from a YAML string.
    pub fn from_str(content: &str) -> Result<Self> {
        let manifest: Manifest = serde_yaml::from_str(content)
            .with_context(|| "failed to parse manifest YAML — check for typos in field names, invalid YAML syntax, or unknown mod_tag values (valid tags: hermes-compat, personalization)")?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Serialize the manifest back to `skills-manifest.yaml` at the repo root.
    ///
    /// This is used to write back updated `base_ref` values after a successful
    /// merge. Note that serde_yaml re-serialization drops any comments that
    /// were present in the original file — this is an accepted trade-off.
    pub fn save(&self, repo_root: &Path) -> Result<()> {
        let manifest_path = repo_root.join("skills-manifest.yaml");
        let yaml =
            serde_yaml::to_string(self).with_context(|| "failed to serialize manifest to YAML")?;
        std::fs::write(&manifest_path, yaml).with_context(|| {
            format!("failed to write manifest file: {}", manifest_path.display())
        })?;
        Ok(())
    }

    /// Validate the manifest, returning an error with a clear message if invalid.
    pub fn validate(&self) -> Result<()> {
        let mut seen_sources = std::collections::HashSet::new();

        for source in &self.sources {
            // Check for duplicate source names
            if !seen_sources.insert(&source.name) {
                bail!(
                    "duplicate source name '{}' — each source must have a unique name",
                    source.name
                );
            }

            let mut seen_skills = std::collections::HashSet::new();
            for skill in &source.skills {
                // Check for duplicate skill paths within a source
                if !seen_skills.insert(&skill.path) {
                    bail!(
                        "duplicate skill path '{}' in source '{}'",
                        skill.path,
                        source.name
                    );
                }

                // Modified skills must have base_ref
                if skill.modified && skill.base_ref.is_none() {
                    bail!(
                        "skill '{}' in source '{}' is marked modified but has no base_ref — \
                         base_ref is required for modified skills to enable 3-way merge",
                        skill.path,
                        source.name
                    );
                }

                // mod_tags on unmodified skills is suspicious — warn but don't error
                if !skill.modified && skill.mod_tags.is_some() {
                    eprintln!(
                        "warning: skill '{}' in source '{}' has mod_tags but is not marked modified \
                         — these tags will be ignored",
                        skill.path,
                        source.name
                    );
                }

                // base_ref on unmodified skills is suspicious — warn but don't error
                if !skill.modified && skill.base_ref.is_some() {
                    eprintln!(
                        "warning: skill '{}' in source '{}' has base_ref but is not marked modified \
                         — this field has no effect on unmodified skills",
                        skill.path,
                        source.name
                    );
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_manifest_unmodified() {
        let yaml = r#"
sources:
  - name: hermes-skills
    skills:
      - path: devops/kanban-orchestrator
        modified: false
"#;
        let manifest = Manifest::from_str(yaml).unwrap();
        assert_eq!(manifest.sources.len(), 1);
        assert_eq!(manifest.sources[0].name, "hermes-skills");
        assert_eq!(manifest.sources[0].skills.len(), 1);
        assert!(!manifest.sources[0].skills[0].modified);
        assert!(manifest.sources[0].skills[0].base_ref.is_none());
    }

    #[test]
    fn test_valid_manifest_modified() {
        let yaml = r#"
sources:
  - name: hermes-skills
    skills:
      - path: mlops/inference/llama-cpp
        modified: true
        base_ref: v1.2.3
        mod_tags: [hermes-compat]
"#;
        let manifest = Manifest::from_str(yaml).unwrap();
        let skill = &manifest.sources[0].skills[0];
        assert!(skill.modified);
        assert_eq!(skill.base_ref.as_deref(), Some("v1.2.3"));
        assert_eq!(
            skill.mod_tags.as_ref().unwrap(),
            &vec![ModTag::HermesCompat]
        );
    }

    #[test]
    fn test_valid_manifest_multiple_sources_and_skills() {
        let yaml = r#"
sources:
  - name: hermes-skills
    skills:
      - path: devops/kanban-orchestrator
        modified: false
      - path: mlops/inference/llama-cpp
        modified: true
        base_ref: v1.2.3
        mod_tags: [hermes-compat]
  - name: another-source
    skills:
      - path: some-skill
        modified: true
        base_ref: abc123
        mod_tags: [hermes-compat, personalization]
"#;
        let manifest = Manifest::from_str(yaml).unwrap();
        assert_eq!(manifest.sources.len(), 2);
        assert_eq!(manifest.sources[0].skills.len(), 2);
        assert_eq!(manifest.sources[1].skills.len(), 1);
    }

    #[test]
    fn test_modified_without_base_ref_errors() {
        let yaml = r#"
sources:
  - name: hermes-skills
    skills:
      - path: some-skill
        modified: true
"#;
        let err = Manifest::from_str(yaml).unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("base_ref"),
            "error should mention base_ref: got: {}",
            msg
        );
        assert!(
            msg.contains("some-skill"),
            "error should mention the skill path: got: {}",
            msg
        );
    }

    #[test]
    fn test_duplicate_source_name_errors() {
        let yaml = r#"
sources:
  - name: hermes-skills
    skills:
      - path: skill-a
        modified: false
  - name: hermes-skills
    skills:
      - path: skill-b
        modified: false
"#;
        let err = Manifest::from_str(yaml).unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("duplicate source name"),
            "error should mention duplicate source: got: {}",
            msg
        );
    }

    #[test]
    fn test_duplicate_skill_path_errors() {
        let yaml = r#"
sources:
  - name: hermes-skills
    skills:
      - path: devops/kanban
        modified: false
      - path: devops/kanban
        modified: false
"#;
        let err = Manifest::from_str(yaml).unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("duplicate skill path"),
            "error should mention duplicate skill path: got: {}",
            msg
        );
    }

    #[test]
    fn test_malformed_yaml_errors() {
        let yaml = r#"
sources:
  - name: hermes-skills
    skills:
      - path: skill-a
        modified: not_a_bool
"#;
        let err = Manifest::from_str(yaml).unwrap_err();
        // Should produce a readable error, not a panic
        assert!(!format!("{}", err).is_empty());
    }

    #[test]
    fn test_empty_sources_list() {
        let yaml = "sources: []";
        let manifest = Manifest::from_str(yaml).unwrap();
        assert!(manifest.sources.is_empty());
    }

    #[test]
    fn test_mod_tags_on_unmodified_warns() {
        let yaml = r#"
sources:
  - name: hermes-skills
    skills:
      - path: skill-a
        modified: false
        mod_tags: [hermes-compat]
"#;
        // Should succeed (warn, not error)
        let manifest = Manifest::from_str(yaml).unwrap();
        assert!(!manifest.sources[0].skills[0].modified);
    }

    #[test]
    fn test_base_ref_on_unmodified_warns() {
        let yaml = r#"
sources:
  - name: hermes-skills
    skills:
      - path: skill-a
        modified: false
        base_ref: v1.0.0
"#;
        // Should succeed (warn, not error)
        let manifest = Manifest::from_str(yaml).unwrap();
        assert!(!manifest.sources[0].skills[0].modified);
    }

    #[test]
    fn test_invalid_mod_tag_errors() {
        let yaml = r#"
sources:
  - name: hermes-skills
    skills:
      - path: some-skill
        modified: true
        base_ref: v1.0.0
        mod_tags: [unknown-tag]
"#;
        let err = Manifest::from_str(yaml).unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("unknown mod_tag values"),
            "error should mention unknown mod_tag: got: {}",
            msg
        );
    }

    #[test]
    fn test_both_mod_tags_valid() {
        let yaml = r#"
sources:
  - name: hermes-skills
    skills:
      - path: some-skill
        modified: true
        base_ref: v1.0.0
        mod_tags: [hermes-compat, personalization]
"#;
        let manifest = Manifest::from_str(yaml).unwrap();
        let tags = manifest.sources[0].skills[0].mod_tags.as_ref().unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0], ModTag::HermesCompat);
        assert_eq!(tags[1], ModTag::Personalization);
    }
}
