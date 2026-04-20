# Feature: Generate third-party license report

## Problem statement

SwiftBeaver depends on many third-party crates (direct + transitive). When distributing the project—especially release binaries—we should be able to quickly produce a complete, reproducible third-party license/notice report derived from `Cargo.lock`.

Right now we only have a lightweight human-maintained list in `THIRD_PARTY_NOTICES.md`. That’s helpful but can drift from the actual resolved dependency set.

## Goals

- Provide a repeatable way to generate a full license inventory for **all resolved dependencies** (transitive included) from `Cargo.lock`.
- Keep output deterministic and offline-friendly (no network required after deps are in the lockfile).
- Make it easy to include the report in release artifacts.

## Scope

- Add a documented workflow to generate a license report.
- Add one of the following implementation approaches (pick one):
  - **Option A (recommended):** Use `cargo-about` with a checked-in `about.toml` and template(s).
  - **Option B (simple):** Use `cargo-license` to dump licenses to text/CSV/JSON.
- Add a script/command entry point (e.g. `scripts/generate-third-party-licenses.sh`) that:
  - Runs the chosen tool
  - Writes output to a predictable location (e.g. `dist/THIRD_PARTY_LICENSES.txt`)
  - Fails clearly if the tool is missing
- Update `README.md` and/or `docs/` with how to generate the report.

## Non-goals

- Changing any runtime behaviour of the scanner/carver.
- Enforcing license policy decisions (e.g. “block GPL”); this feature only reports what’s present.
- Replacing `THIRD_PARTY_NOTICES.md` entirely (we can keep it as a short human-facing pointer).

## Design notes

### Output format

- Default output: `dist/THIRD_PARTY_LICENSES.txt` (easy to ship alongside binaries).
- Optional additional output: `dist/THIRD_PARTY_LICENSES.html` (nice for humans).

### Tool choice

**Option A: `cargo-about`**
- Pros: Produces a comprehensive NOTICE-style output including license texts; strong templating; commonly used for compliance.
- Cons: Requires an `about.toml` config and template file(s).

**Option B: `cargo-license`**
- Pros: Very quick to wire in; great for inventories.
- Cons: Typically focuses on listing metadata rather than embedding full license texts; may require extra steps if we want full license text bundles.

### Where to document

- Keep the README short: link to a dedicated doc.
- Add a new doc page, e.g. `docs/third_party_licenses.md`:
  - When to generate (before release)
  - Command(s) to run
  - Where the output goes
  - How to include it in release artifacts

## Expected tests

- Minimal:
  - If we add a script, add a lightweight test that checks the script exists and is executable (Linux/macOS), or at least that it prints a helpful error when the tool is not installed.
- Optional:
  - CI job or local check that runs the generator when the tool is available.

## Impact on documentation

- Update `README.md` to point to the generation instructions.
- Add `docs/third_party_licenses.md` describing the process.

## Open questions

- Which tool do we prefer (`cargo-about` vs `cargo-license`)?
- Do we want to check in the generated output (for releases), or generate during release packaging only?
- Do we want output to include full license texts, or only an inventory list?
