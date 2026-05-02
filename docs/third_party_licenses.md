# Third-Party License Reports

SwiftBeaver keeps a short human-maintained notice file in
`THIRD_PARTY_NOTICES.md`. For release artifacts, generate the full dependency
license report from the resolved Cargo dependency graph.

## When to Generate

Generate `dist/THIRD_PARTY_LICENSES.txt` before packaging or publishing release
binaries. The report is intended to be shipped next to the binary artifacts for
that release.

## Prerequisites

Install `cargo-about` once:

```bash
cargo install --locked cargo-about --features cli
```

If Cargo installs the binary but the script still cannot find it, add Cargo's
bin directory to your shell `PATH`:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

The generator requires a `Cargo.lock` in the repository root. It runs
`cargo-about` with `--all-features` and `--locked` so the command uses the
existing lockfile and fails instead of changing dependency resolution. The
checked-in `about.toml` also lists the release target families to keep
target-specific dependencies consistent across generator hosts.

## Generate the Report

From the repository root:

```bash
scripts/generate-third-party-licenses.sh
```

The script writes:

```text
dist/THIRD_PARTY_LICENSES.txt
```

`dist/` is intentionally ignored by git. Regenerate the report during release
packaging and include the generated text file with the release artifacts.

## Offline Use

After dependencies and the crates.io index entries needed by `Cargo.lock` are
present locally, generation does not require network access. To force offline
mode, run the generator with Cargo's offline environment setting:

```bash
CARGO_NET_OFFLINE=true scripts/generate-third-party-licenses.sh
```

If the script fails because dependencies are not available offline, run
`cargo fetch --locked --all-features` in a networked environment first, then rerun
the generator.
