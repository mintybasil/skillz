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
///
/// The same source name may appear multiple times with different `base_path`
/// values to scope skills to different subdirectories within the submodule.
#[derive(Debug, Deserialize, Serialize)]
pub struct Source {
    pub name: String,
    /// Upstream ref that modifications are based on. Required when any skill in
    /// this source is marked `modified`. Serves as the base for 3-way merge.
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub ref_: Option<String>,
    /// Subdirectory within the source repo where skills are located.
    /// When set, skill paths are relative to `sources/<name>/<base_path>/<skill_path>`.
    /// When empty or unset, skill paths are relative to `sources/<name>/<skill_path>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_path: Option<String>,
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
    /// This is used to write back updated `ref` values after a successful
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
        let mut seen_source_keys = std::collections::HashSet::new();

        for source in &self.sources {
            // Check for duplicate (name, base_path) combinations
            let base_path_key = source.base_path.as_deref().unwrap_or("");
            let source_key = (&source.name, base_path_key);
            if !seen_source_keys.insert(source_key) {
                bail!(
                    "duplicate source name '{}' with base_path '{}' — \
                     each (name, base_path) combination must be unique",
                    source.name,
                    if base_path_key.is_empty() {
                        "(empty)"
                    } else {
                        base_path_key
                    }
                );
            }

            // If any skill in this source is modified, the source must have ref set
            let has_modified = source.skills.iter().any(|s| s.modified);
            if has_modified && source.ref_.is_none() {
                bail!(
                    "source '{}' has modified skills but no ref — \
                     ref is required on the source when any skill is modified, \
                     to enable 3-way merge",
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

                // mod_tags on unmodified skills is suspicious — warn but don't error
                if !skill.modified && skill.mod_tags.is_some() {
                    eprintln!(
                        "warning: skill '{}' in source '{}' has mod_tags but is not marked modified \
                         — these tags will be ignored",
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
        assert!(manifest.sources[0].ref_.is_none());
    }

    #[test]
    fn test_valid_manifest_modified() {
        let yaml = r#"
sources:
  - name: hermes-skills
    ref: v1.2.3
    skills:
      - path: mlops/inference/llama-cpp
        modified: true
        mod_tags: [hermes-compat]
"#;
        let manifest = Manifest::from_str(yaml).unwrap();
        let skill = &manifest.sources[0].skills[0];
        assert!(skill.modified);
        assert_eq!(manifest.sources[0].ref_.as_deref(), Some("v1.2.3"));
        assert_eq!(
            skill.mod_tags.as_ref().unwrap(),
            &vec![ModTag::HermesCompat]
        );
    }

    #[test]
    fn test_valid_manifest_with_base_path() {
        let yaml = r#"
sources:
  - name: hermes-skills
    ref: v1.2.3
    base_path: skills
    skills:
      - path: devops/kanban-orchestrator
        modified: false
      - path: mlops/inference/llama-cpp
        modified: true
        mod_tags: [hermes-compat]
"#;
        let manifest = Manifest::from_str(yaml).unwrap();
        assert_eq!(manifest.sources[0].base_path.as_deref(), Some("skills"));
        assert_eq!(manifest.sources[0].skills.len(), 2);
    }

    #[test]
    fn test_valid_manifest_multiple_sources_and_skills() {
        let yaml = r#"
sources:
  - name: hermes-skills
    ref: v1.2.3
    skills:
      - path: devops/kanban-orchestrator
        modified: false
      - path: mlops/inference/llama-cpp
        modified: true
        mod_tags: [hermes-compat]
  - name: another-source
    ref: abc123
    skills:
      - path: some-skill
        modified: true
        mod_tags: [hermes-compat, personalization]
"#;
        let manifest = Manifest::from_str(yaml).unwrap();
        assert_eq!(manifest.sources.len(), 2);
        assert_eq!(manifest.sources[0].skills.len(), 2);
        assert_eq!(manifest.sources[1].skills.len(), 1);
    }

    #[test]
    fn test_duplicate_source_name_different_base_path_ok() {
        let yaml = r#"
sources:
  - name: hermes-skills
    base_path: skills
    skills:
      - path: skill-a
        modified: false
  - name: hermes-skills
    base_path: tools
    skills:
      - path: skill-b
        modified: false
"#;
        let manifest = Manifest::from_str(yaml).unwrap();
        assert_eq!(manifest.sources.len(), 2);
    }

    #[test]
    fn test_duplicate_source_name_same_base_path_errors() {
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
    fn test_modified_without_source_ref_errors() {
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
            msg.contains("ref"),
            "error should mention ref: got: {}",
            msg
        );
        assert!(
            msg.contains("hermes-skills"),
            "error should mention source name: got: {}",
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
    fn test_invalid_mod_tag_errors() {
        let yaml = r#"
sources:
  - name: hermes-skills
    ref: v1.0.0
    skills:
      - path: some-skill
        modified: true
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
    ref: v1.0.0
    skills:
      - path: some-skill
        modified: true
        mod_tags: [hermes-compat, personalization]
"#;
        let manifest = Manifest::from_str(yaml).unwrap();
        let tags = manifest.sources[0].skills[0].mod_tags.as_ref().unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0], ModTag::HermesCompat);
        assert_eq!(tags[1], ModTag::Personalization);
    }
}
