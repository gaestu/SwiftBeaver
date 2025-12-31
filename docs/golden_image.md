# Golden Image Tests

This project includes an optional "golden" disk image that packs every file
under `tests/golden_image/samples/` for end-to-end carving tests.

## Generate the golden image

```bash
cd tests/golden_image
./generate.sh           # raw + E01 (requires ewfacquire)
./generate.sh --no-e01  # raw only
```

The generator produces:

- `tests/golden_image/golden.raw`
- `tests/golden_image/golden.E01` (when `ewfacquire` is available)
- `tests/golden_image/manifest.json`

`golden.raw` is ignored by git; `golden.E01` can be committed for convenience.

## Manifest format

`manifest.json` is the source of truth for test expectations. Each entry records
path, category, offset, size, and sha256, plus a summary of totals.

## Tests

```bash
cargo test golden
cargo test golden --features ewf
```

Tests skip automatically if `golden.raw`, `golden.E01`, or `manifest.json` are
missing.
