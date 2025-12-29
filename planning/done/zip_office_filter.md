Status: Implemented
Implemented in version: 0.1.0

# Office ZIP Type Filtering

## Problem statement
`--types docx/xlsx/pptx` are treated as unknown types, so ZIP-derived Office files cannot be targeted or excluded cleanly.

## Scope
- Recognize docx/xlsx/pptx in type filters.
- Allow ZIP carving to output only selected Office subtypes when filtered.

## Non-goals
- New ZIP signatures or validators.
- Deep Office document parsing.

## Design notes
- Keep ZIP signatures as the only scanner patterns.
- Track allowed ZIP kinds from `--types` and apply filtering in the ZIP handler.
- Treat `--types zip` as allowing all ZIP-derived types.

## Expected tests
- `filter_file_types` accepts docx and retains ZIP handler.
- ZIP handler skips non-allowed kinds.

## Impact on docs and README
- Update CLI `--types` description.
- Document the `zip_allowed_kinds` config field.
