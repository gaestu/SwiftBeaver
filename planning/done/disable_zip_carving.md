Status: Implemented
Implemented in version: 0.1.0

# Disable ZIP Carving CLI Flag

Short description: Add a CLI switch to disable ZIP carving to avoid excessive output during testing.

## Problem statement
ZIP carving can generate large amounts of output (including false positives) and quickly fill disk space during exploratory runs. Users need a way to disable ZIP carving without modifying config files.

## Scope
- Add a CLI flag to disable ZIP carving.
- Ensure the flag removes ZIP handlers from active file types at runtime.
- Document the flag in `README.md`.
- Add tests for CLI parsing and behavior gating.

## Non-goals
- Changing ZIP validation heuristics beyond existing configuration options.
- Modifying ZIP output formats or metadata schemas.

## Design notes
- Implement a `--disable-zip` boolean flag in the CLI.
- When set, filter `cfg.file_types` to exclude `zip` before pipeline start.
- Leave `config/default.yml` unchanged aside from existing ZIP validation options.

## Expected tests
- CLI parsing test that confirms `--disable-zip` sets the flag.

## Impact on docs and README
- Add the new flag to `README.md` usage section.
- No new `/docs` page required.
