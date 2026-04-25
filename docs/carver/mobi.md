# MOBI Carver

## Overview

The MOBI carver extracts Mobipocket and Kindle-era ebook files stored in a Palm Database (PDB) container. In SwiftBeaver, this handler targets PDB-based ebook variants such as `.mobi`, `.azw`, and related legacy Kindle payloads that expose the `BOOKMOBI` identifier.

Rather than searching for a footer, the carver uses the PDB record table to estimate the extent of the file and then copies the contiguous byte range from the true container start.

## Signature Detection

**Primary signature**: `BOOKMOBI` at byte offset 60 from the start of the PDB container

- Hex: `42 4F 4F 4B 4D 4F 42 49`
- ASCII: `BOOKMOBI`

The scanner is configured to hit on the `BOOKMOBI` magic itself, not on byte 0 of the file. The carver therefore normalizes each hit by subtracting 60 bytes so validation and extraction begin at the actual PDB header start.

This is a stronger signal than a generic PalmDOC/PDB header alone because the PDB container is reused by multiple Palm-era formats.

## Carving Algorithm

SwiftBeaver uses a bounded structural heuristic instead of full MOBI parsing.

### 1. Normalize to the PDB header

When the scanner reports a `mobi_pdb` hit, the handler subtracts 60 bytes from the hit offset. Hits that would place the start before offset 0 are rejected.

### 2. Read and validate the fixed PDB header

The handler reads the first 78 bytes of the container and checks:

- `BOOKMOBI` is present at bytes 60 to 67
- The PDB record count at bytes 76 to 77 is non-zero
- The record count is not implausibly large (`<= 4096`)

### 3. Read the PDB record table

Each PDB record entry is 8 bytes long. SwiftBeaver reads the full record list immediately after the fixed header and extracts the 4-byte big-endian record offsets.

### 4. Sanity-check the first record

The earliest record must begin after the fixed header and record table. If the first record overlaps the header region, the hit is rejected as a false positive or damaged structure.

### 5. Estimate file extent

The handler sorts the record offsets and estimates the total file size as:

$$
\text{estimated size} = \text{last record offset} + \max(1,\ \text{last record offset} - \text{previous record offset})
$$

If only one record is present, the handler falls back to a conservative 4096-byte estimate for the final record length.

The resulting extent is then capped by the configured `max_size`.

### 6. Copy and hash

The handler writes the computed byte range to the designated output directory and computes MD5 and SHA-256 incrementally when those hashes are enabled.

### 7. Mark validation state

If the entire estimated range is available in the evidence, the output is marked `validated = true`. If EOF is reached before the estimated end, the file is still emitted when it meets `min_size`, but it is marked truncated.

## Validation

Validation is structural and intentionally lightweight. The carver does **not** parse the full MOBI header, decompress text records, or interpret ebook metadata fields before carving.

Current acceptance checks are:

1. The hit must normalize to a non-negative PDB start offset.
2. The 78-byte PDB header must be readable.
3. The `BOOKMOBI` magic must appear at offset 60.
4. The PDB record count must be within a plausible range (`1..=4096`).
5. The record table must be fully readable.
6. The first record offset must not overlap the header or record list.

These checks reject many random `BOOKMOBI` byte sequences while still allowing partially damaged ebooks to be carved when the top-level structure is intact. They are not a full defense against every malformed record table.

## Size Constraints

- **Default min_size**: 68 bytes
- **Default max_size**: 512 MiB (`536870912` bytes)
- Configured in `config/default.yml` under the `mobi` file type entry
- Default extensions: `mobi`, `azw`, `azw3`, `prc`

The minimum size is aligned with the requirement that the fixed header and magic region be present. The maximum size is a safety cap on the estimated extent derived from the record table.

## Hash Computation

- **MD5**: Computed incrementally during extraction when enabled
- **SHA-256**: Computed incrementally during extraction when enabled

Hashes cover exactly the bytes written to the carved output, including any truncated output that ends at EOF before the estimated extent.

## Testing

Current coverage is concentrated in unit tests for the handler implementation:

- `src/carve/mobi.rs` includes deterministic tests for a minimal valid PDB/MOBI structure
- Pre-validation accepts a valid `BOOKMOBI` header
- Pre-validation rejects invalid record counts
- Extraction normalizes a hit at offset 60 back to the true file start and carves the expected byte count

These tests exercise the core structural checks without depending on a large external ebook corpus.

## Edge Cases

### KF7 vs KF8 hybrid files

Some Kindle ebooks combine legacy MOBI/KF7 content with newer KF8 content inside a hybrid package. SwiftBeaver does not attempt to split or semantically distinguish those layers. It carves the outer PDB-based container as one artifact when the `BOOKMOBI` structure is present.

### DRM-protected files

DRM-protected MOBI or Kindle files are carved as raw evidence. SwiftBeaver does not attempt decryption, license validation, or format-specific unpacking of protected content.

### Damaged record tables

If the record count is implausible, the record list is truncated, or the first record overlaps the header area, the hit is rejected.

Later record offsets are not deeply validated beyond being readable from the record table. A malformed but plausible table can therefore still cause the handler to over-estimate file extent up to EOF or the configured `max_size` cap.

### Single-record files

When only one record is present, the carver cannot infer the final record length from a following offset. In that case it uses a fixed 4096-byte estimate, capped by `max_size` and EOF.

## Performance

- **Read pattern**: One small header read, one record-table read, then a sequential copy of the estimated extent
- **Memory use**: Low; proportional to the PDB record table size plus normal I/O buffers
- **Complexity**: $O(n \log n)$ for sorting record offsets, then linear in bytes written
- **Queue placement**: Classified as a fast carver (`is_fast() == true`)

In practice, the structural work is cheap compared with large streaming formats because most MOBI files have modest record counts and require only a small amount of metadata parsing before extraction.

## Forensic Considerations

MOBI files commonly retain useful attribution and catalog metadata inside the ebook headers and records, including:

- Author name
- Book title
- ASIN or Kindle-specific identifiers
- Publisher and language fields
- Embedded cover images or auxiliary content

SwiftBeaver preserves the raw container bytes exactly as carved. It does not rewrite metadata, normalize text encodings, or strip DRM-related structures. Standard provenance fields remain available through normal carved-file reporting, including `run_id`, `tool_version`, `config_hash`, and `evidence_path`.

## Structure Examples

At a high level, the handler depends on this container layout:

```text
Offset  Size  Meaning
0       78    Palm Database header
60      8     MOBI identifier: BOOKMOBI
76      2     Record count (big-endian)
78      8*n   Record table entries
...           Record payload data
```

Conceptual record-table example:

```text
PDB header
  record_count = 3

Record table
  record[0] -> offset 0x00000080
  record[1] -> offset 0x00001000
  record[2] -> offset 0x00002800

Estimated extent
  last_offset = 0x2800
  inferred_last_record_size = 0x2800 - 0x1000 = 0x1800
  total_size = 0x2800 + 0x1800 = 0x4000
```

Minimal signature example:

```text
... 60 bytes of PDB header prefix ...
42 4F 4F 4B 4D 4F 42 49
 B  O  O  K  M  O  B  I
```

## Known Limitations

- The handler estimates extent from record offsets rather than parsing every record to a verified terminus
- Later record offsets are not deeply validated, so malformed tables can still lead to over-carving up to EOF or `max_size`
- No deep validation of MOBI headers, EXTH metadata blocks, compression settings, or text/image records
- No separation of KF7 and KF8 substructures in hybrid Kindle files
- No decryption or DRM-aware interpretation
- Newer non-PDB Kindle containers are outside the scope of this handler unless they still present a compatible `BOOKMOBI`/PDB structure

## Related Carvers

- **FB2**: XML-based ebook format documented in `fb2.md`
- **LRF**: Sony BBeB ebook format documented in `lrf.md`
