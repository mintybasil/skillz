# skills-managed

A tool for managing agent skills across multiple deployments. Syncs skills from external git submodule sources into a bundle directory, with 3-way merge support for locally-modified skills.

## How it works

External skill sources are tracked as git submodules under `sources/`. A manifest file (`skills-manifest.yaml`) defines which skills to include from each source and tracks modification metadata. The tool reads the manifest and copies/merges selected skills into the `skills/` bundle directory.

For skills that have been locally modified (e.g., for Hermes harness compatibility or personalization), the tool performs a 3-way merge using `git merge-file` when upstream changes are detected — preserving local modifications while incorporating upstream updates.

## Quick start

```bash
# Clone the repo with submodules
git clone --recurse-submodules https://github.com/mintybasil/skills-managed.git
cd skills-managed

# Add an external skill source as a submodule
git submodule add https://github.com/some-org/skills-repo sources/my-source

# Create a manifest (or use `cargo run -- init`)
# Edit skills-manifest.yaml to select skills from your source

# Sync the bundle
cargo run -- sync
```

## Manifest format

```yaml
sources:
  - name: my-source              # maps to submodule path sources/my-source
    skills:
      - path: category/skill-a   # path within the source repo
        modified: false

      - path: category/skill-b
        modified: true
        base_ref: v1.0.0         # upstream ref modifications are based on
        mod_tags: [hermes-compat] # nature of modifications
```

Locally-created skills go in `skills/mintybasil/` and are included automatically — no manifest entry needed.

## Usage

```bash
cargo run -- sync    # Build/rebuild the skill bundle
cargo run -- init    # Create minimal manifest and directory structure
```

## License

MIT