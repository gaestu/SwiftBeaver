# Third-Party Notices

SwiftBeaver is distributed under the Apache License, Version 2.0.

This project depends on third-party open-source components. License obligations for those components apply when you redistribute this project (especially binaries).

## Direct dependencies

Below is a human-maintained list of the **direct** Rust crate dependencies used by this repository (see Cargo.toml). Licenses shown are the SPDX identifiers as declared by each crate.

- anyhow — MIT OR Apache-2.0
- arrow-array — Apache-2.0
- arrow-schema — Apache-2.0
- chrono — MIT OR Apache-2.0
- clap — MIT OR Apache-2.0
- crossbeam-channel — MIT OR Apache-2.0
- ctrlc — MIT OR Apache-2.0
- csv — MIT OR Apache-2.0
- hex — MIT OR Apache-2.0
- libc — MIT OR Apache-2.0
- md5 — MIT OR Apache-2.0
- memchr — Unlicense OR MIT
- num_cpus — MIT OR Apache-2.0
- once_cell — MIT OR Apache-2.0
- opencl3 (optional) — Apache-2.0
- cudarc (optional) — MIT
- parquet — Apache-2.0
- regex — MIT OR Apache-2.0
- rusqlite — MIT
- serde — MIT OR Apache-2.0
- serde_json — MIT OR Apache-2.0
- serde_yaml — MIT OR Apache-2.0
- sha2 — MIT OR Apache-2.0
- thiserror — MIT OR Apache-2.0
- tracing — MIT
- tracing-subscriber — MIT

Dev-dependencies:

- tempfile — MIT OR Apache-2.0
- criterion — MIT OR Apache-2.0

## Transitive dependencies

Rust projects typically include many transitive dependencies.

- The complete resolved dependency set is recorded in Cargo.lock.
- For a full, up-to-date license inventory, generate a report from the lockfile.

### Suggested tooling (optional)

You can generate a full dependency/license report using one of these common tools:

- cargo-license: https://github.com/onur/cargo-license
- cargo-about: https://github.com/EmbarkStudios/cargo-about

(If you add one of these tools to your workflow, consider checking in the generated output alongside this file for release builds.)
