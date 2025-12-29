Status: Implemented
Implemented in version: 0.1.0

# EWF (E01) Default Support

## Problem statement
E01 support is behind a feature flag, so default builds do not handle EWF images.

## Scope
- Enable the `ewf` feature by default.
- Document libewf requirements and how to build without E01 support.

## Non-goals
- Implement new EWF parsing logic beyond the existing libewf bindings.
- Automatic download/installation of libewf.

## Design notes
- Update Cargo features to include `ewf` in `default`.
- Update README/docs to reflect the new default.

## Expected tests
- Existing evidence source tests should continue to pass.

## Impact on docs and README
- Update README build/run instructions for E01 input.
- Update architecture docs to note E01 support is default (libewf required).
