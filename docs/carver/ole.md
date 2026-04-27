# OLE / CFB Carver

## Overview

The OLE carver extracts Microsoft **Compound File Binary** (CFB, also called
OLE2 / Structured Storage) containers from raw forensic evidence. CFB is the
underlying storage format used by the legacy Microsoft Office 97–2003 family
and several related artefacts:

| Extension | Format | Distinguishing storage stream |
|-----------|--------|-------------------------------|
| `doc` | Word 97–2003 document | `WordDocument` |
| `xls` | Excel 97–2003 workbook | `Workbook` (also legacy `Book`) |
| `ppt` | PowerPoint 97–2003 presentation | `PowerPoint Document` |
| `msg` | Outlook saved message | (carved as generic `ole`) |
| `ole` | Other CFB containers (Visio, Project, MSI, installers) | none of the above |

When SwiftBeaver can identify the embedded application by inspecting the root
storage's directory entries, the carved file is renamed and reclassified to the
matching subtype (`doc`, `xls`, `ppt`). Otherwise the file is emitted under the
generic `ole` type and `ole` extension.

The handler implementation lives in [src/carve/ole.rs](src/carve/ole.rs).

## Signature Detection

**Header pattern**: `D0 CF 11 E0 A1 B1 1A E1` (8 bytes)

This eight-byte magic appears at the very start of every CFB container,
regardless of major version (3 or 4) and regardless of which Office
application produced it. The carver requires an exact match before any header
parsing is attempted.

### Config Pattern

```yaml
- id: "ole"
  extensions: ["ole"]
  header_patterns:
    - id: "ole_cfb"
      hex: "D0CF11E0A1B11AE1"
  footer_patterns: []
  max_size: 104857600   # 100 MiB
  min_size: 512
  validator: "ole"
```

The optional `ole_allowed_kinds` top-level config setting restricts which
classified subtypes are kept (e.g. `["doc", "xls"]` to drop everything else).
Unset means all classified and unclassified CFB containers are kept.

## Carving Algorithm

CFB is **structure-based** rather than marker-based: there is no end-of-file
sentinel to scan for. Instead, the file extent is computed by walking the
sector-allocation tables (FAT) referenced from the header.

### 1. Header Validation

A 512-byte header is read and validated:

| Bytes | Field | Required value |
|-------|-------|----------------|
| 0–7   | Signature | `D0 CF 11 E0 A1 B1 1A E1` |
| 26–27 | Major version (LE u16) | `3` (512-byte sectors) or `4` (4096-byte sectors) |
| 28–29 | Byte-order mark | `0xFFFE` (little-endian) |
| 30–31 | Sector size power | `9` for v3 (2⁹ = 512), `12` for v4 (2¹² = 4096) |
| 44–47 | Number of FAT sectors | ≤ `MAX_FAT_SECTORS` (1000) |
| 48–51 | First directory sector ID | informative |
| 64–67 | First DIFAT sector ID | informative |
| 68–71 | Number of DIFAT sectors | informative |
| 76–511 | DIFAT array (109 × u32) | sector IDs of FAT sectors |

If any of these checks fail the candidate is rejected (`PreValidation::Reject`
or `CarveError::Invalid`), the partial output is discarded, and no file is
written.

### 2. Initial Size Estimate

`parse_ole_header` computes an upper bound on the file extent using:

- the number of FAT sectors and their addressable capacity
  (`num_fat_sectors × sector_size / 4`)
- the highest FAT sector ID found in the in-header DIFAT array
- the first directory sector ID
- the count of DIFAT continuation sectors

This estimate is intentionally generous; it is refined in step 3.

### 3. FAT-Walk Refinement

`refine_ole_size` reads each FAT sector referenced from the in-header DIFAT
(continuation DIFAT sectors are not chased) and scans every FAT entry to find
the highest **allocated** sector index. A sector is considered allocated
unless its FAT entry equals `FREESECT` (`0xFFFFFFFF`). Special chain
terminators (`ENDOFCHAIN`, `FATSECT`, `DIFSECT`) are treated as in-use.

The carved extent is then:

```
size = 512 (header) + (highest_used_sector + 1) × sector_size
```

clamped to the configured `max_size`.

#### Garbage-Resistant Fallback

False-positive headers in random data tend to produce FAT sectors filled with
pointers outside the addressable range. When the count of out-of-range entries
exceeds the count of plausible entries, the FAT is treated as corrupt and a
**conservative** size is used instead:

```
size = 512 + (num_fat_sectors_in_header + 2) × sector_size
```

This bounds the damage of a false positive while still letting genuinely
truncated CFB containers be recovered.

### 4. Subtype Classification

Once a size is known, `classify_ole_kind` walks the directory chain starting
at `first_dir_sector`, decoding 128-byte directory entries. The UTF-16LE name
of each storage entry (entry type `0x02`) is checked against:

| Storage name | Subtype |
|--------------|---------|
| `WordDocument` | `doc` |
| `Workbook` or `Book` | `xls` |
| `PowerPoint Document` | `ppt` |

The first match wins (`doc` beats `xls` beats `ppt` if multiple are present,
which is rare). Walks are bounded to 1024 visited directory sectors to defend
against malformed chains.

### 5. Streaming and Rename

The carver streams the computed `target_size` bytes through the standard
`CarveStream` writer (which computes hashes incrementally). If a subtype was
classified, the in-flight output is flushed and renamed from
`carved/ole/...ole` to `carved/<subtype>/...<subtype>` before the metadata row
is emitted. If renaming fails the file remains as the generic `ole` output.

After streaming, `ole_allowed_kinds` (if configured) is enforced: containers
whose final classified type is not in the allow-list are discarded.

## Validation

| Outcome | Condition |
|---------|-----------|
| `validated = true` | header parsed, FAT walked, requested extent fully read |
| `truncated = true` | EOF reached before `target_size`, or `max_size` clamp hit |
| Discarded | header signature/byte-order/version/sector-power invalid, or final size below `min_size` |

Validation never executes embedded macros, opens streams, or otherwise
interprets document content; it only proves that the byte range is a
plausible CFB container.

## Size Constraints

- **Default `min_size`**: 512 bytes (one sector). A CFB cannot be smaller than
  its header.
- **Default `max_size`**: 100 MiB (`104857600`). When `max_size` is set to 0
  the handler falls back to an internal 100 MiB cap.
- **Hard FAT cap**: `MAX_FAT_SECTORS = 1000`. Headers claiming more FAT
  sectors are rejected up-front to bound work for false positives.
- **Directory walk cap**: classification visits at most 1024 directory
  sectors per candidate.

## Hash Computation

MD5 and SHA-256 are computed incrementally by `CarveStream` as the carved
range is read. The hashes cover exactly the bytes written to the output file
(header through the FAT-derived end of the container, or up to the truncation
point). No hash is recomputed after a subtype rename — the file content is
unchanged.

## Testing

**Test file**: [tests/carver_ole.rs](tests/carver_ole.rs)

Coverage:

- Golden-image regression: `finds_all_ole_files` asserts that every
  manifest entry of type `doc`, `xls`, `ppt`, or `msg` is recovered with the
  expected offset and size.
- Unit tests inside `src/carve/ole.rs`:
  - `parses_ole_header` — header parser succeeds on a hand-built minimal CFB.
  - `rejects_invalid_signature` — corrupt magic is rejected.
  - `carves_minimal_ole` — end-to-end carve through the handler.

The minimal-CFB fixture (`create_minimal_ole`) is a useful reference for
constructing synthetic compound files in additional tests.

## Edge Cases

1. **False-positive headers in random data** — the FAT-corruption fallback
   (see [Carving Algorithm § 3](#3-fat-walk-refinement)) bounds the carved
   extent so that a single chance match of `D0 CF 11 E0 A1 B1 1A E1` does not
   trigger a megabyte-scale write. See related work tracked in
   [#13](https://github.com/hugoatease/SwiftBeaver/issues/13).
2. **Header corruption with valid signature** — invalid version, byte order,
   or sector-power values cause `parse_ole_header` to return
   `CarveError::Invalid` and the candidate is discarded.
3. **Truncated container at end of evidence** — partial reads are tolerated;
   the file is kept and marked `truncated = true`.
4. **DIFAT continuation sectors** — currently not followed. Files larger than
   the 109 in-header DIFAT entries × FAT capacity may be under-sized. The
   conservative cap on `MAX_FAT_SECTORS` keeps this bounded.
5. **Mini-FAT streams** — short streams stored in the mini-FAT do not affect
   extent computation; they live inside sectors that the FAT walk already
   covers.
6. **Unknown CFB subtypes** (Visio, MSI, MSG, etc.) — kept as the generic
   `ole` type with extension `ole`.
7. **Multiple known storages in one container** — first match wins
   (`doc` > `xls` > `ppt`).

## Performance

- **I/O pattern**: one 512-byte header read, then one read per referenced
  FAT sector during refinement, then a single sequential read of the carved
  extent. Random reads are bounded by `MAX_FAT_SECTORS` (≤ 1000 sectors).
- **Memory**: one FAT-sector buffer (512 B or 4 KiB) plus the `CarveStream`
  output buffer; classification allocates one sector buffer per directory
  sector visited (capped at 1024).
- **Complexity**: dominated by the FAT walk
  (`O(num_fat_sectors × entries_per_sector)`), which is bounded by the
  capacity cap.

## Forensic Considerations

- Source evidence is opened read-only; the carver never writes back.
- Provenance fields (`run_id`, `tool_version`, `config_hash`,
  `evidence_path`) are populated by the surrounding pipeline and persisted
  on every metadata row.
- Hashes (`md5`, `sha256`) cover the exact carved byte range, enabling
  independent verification.
- Truncated containers are preserved (with `truncated = true`) so analysts
  can attempt manual repair instead of silently dropping evidence.
- Subtype classification is metadata-only — no embedded scripts, OLE
  automation, or macros are executed.

## Structure Examples

### CFB v3 (512-byte sectors) layout

```
+--------------------------+ offset 0
| Header (512 bytes)       |
|  - signature D0 CF 11 E0 |
|  - version 3, BOM 0xFFFE |
|  - DIFAT[109] sector IDs |
+--------------------------+ offset 512
| Sector 0                 |
+--------------------------+
| Sector 1                 |
+--------------------------+
| ...                      |
+--------------------------+
| Sector N (highest used)  |
+--------------------------+ end of carved range
```

### Directory entry (128 bytes, simplified)

```
0   64  | UTF-16LE name (<=64 bytes, NUL-terminated)
64  66  | Name length in bytes (incl. terminator)
66  67  | Entry type (0=empty, 1=storage, 2=stream, 5=root)
67  68  | Color flag (red/black tree)
68  76  | Left, right, child sibling IDs
76  92  | CLSID
...     | Stream start sector, stream size, etc.
```

## Known Limitations

- **DIFAT continuation chains are not followed.** Containers with more than
  109 FAT sectors may be carved short. In practice this affects very large
  (>~7 MiB for v3, much larger for v4) compound files only.
- **Mini-FAT extents are not analysed.** Carved files include them implicitly
  via FAT coverage but they are not separately verified.
- **Embedded subtype detection is name-based.** A container that has been
  obfuscated by renaming `WordDocument` etc. will fall back to the generic
  `ole` type.
- **MSG, Visio, Project, MSI** are recognised as CFB containers but are not
  classified to dedicated extensions.
- **Encrypted compound files** are carved as opaque containers; no
  decryption is attempted.

## Related Carvers

- [PDF](pdf.md) — other primary document format; marker-based rather than
  structure-based.
- [RTF](rtf.md) — text-based document format often produced alongside
  legacy Office files.
- [ZIP](zip.md) — container for the modern OOXML formats (`docx`, `xlsx`,
  `pptx`) that superseded CFB-based Office files.
