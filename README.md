# skillz

A tool for managing agent skills across multiple deployments. Syncs skills from external git submodule sources into a bundle directory, with 3-way merge support for locally-modified skills.

## How it works

External skill sources are tracked as git submodules under `sources/`. A manifest file (`skills-manifest.yaml`) defines which skills to include from each source and tracks modification metadata. The tool reads the manifest and copies/merges selected skills into the `skills/` bundle directory.

For skills that have been locally modified (e.g., for Hermes harness compatibility or personalization), the tool performs a 3-way merge using `git merge-file` when upstream changes are detected — preserving local modifications while incorporating upstream updates.

## Quick start

```bash
# Clone the repo with submodules
git clone --recurse-submodules https://github.com/mintybasil/skillz.git
cd skillz

# Add an external skill source as a submodule
git submodule add https://github.com/some-org/skills-repo sources/my-source

# Create a manifest from the example
cp skills-manifest.yaml.example skills-manifest.yaml
# Edit skills-manifest.yaml to select skills from your source

# Sync the bundle
cargo run -- sync
```

## Manifest format

```yaml
sources:
  - name: my-source              # maps to submodule path sources/my-source
    ref: v1.0.0                   # upstream ref modifications are based on (required if any skill is modified)
    base_path: skills             # optional: subdirectory in the source repo where skills live
    skills:
      - path: category/skill-a   # path within the source repo (relative to base_path if set)
        modified: false

      - path: category/skill-b
        modified: true
        mod_tags: [hermes-compat] # nature of modifications
```

### Fields

- **`name`**: Maps to the submodule path `sources/<name>`. The same name may appear multiple times with different `base_path` values.
- **`ref`**: The upstream ref that modifications are based on. Required when any skill in the source is marked `modified`. Serves as the base for 3-way merge. Updated after clean merges.
- **`base_path`** (optional): Subdirectory within the source repo where skills are located. When set, skill paths are relative to `sources/<name>/<base_path>/<skill_path>`. When empty or unset, skill paths are relative to `sources/<name>/<skill_path>`.
- **`path`**: Path within the source repo (relative to `base_path` if set).
- **`modified`**: Whether this skill has local modifications.
- **`mod_tags`**: Tags describing the nature of modifications (e.g., `hermes-compat`, `personalization`).

Locally-created skills go in `skills/mintybasil/` and are included automatically — no manifest entry needed.

## Usage

```bash
cargo run -- sync                        # Build/rebuild the skill bundle
cargo run -- sync --source hermes-skills  # Print current HEAD SHA for a source
cargo run -- import sources/my-source     # Import skills from a path into the manifest
cargo run -- lint                         # Validate the manifest and check for issues
cargo run -- init                         # Create minimal manifest and directory structure
```

## Documentation

- [PRD](docs/PRD.md) — Product requirements document
- [Example manifest](skills-manifest.yaml.example) — Annotated manifest template

## License

MIT