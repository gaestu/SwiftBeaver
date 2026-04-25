# ELF Carver

## Overview

The ELF carver extracts **Executable and Linkable Format** binaries from raw
forensic evidence. ELF is the standard binary format used by Linux, BSD, and
many embedded/Unix systems for:

- Executables (`/bin/ls`, `a.out`)
- Shared objects (`.so` libraries)
- Loadable kernel modules (`.ko`)
- Core dumps
- Object files (`.o`)

Both 32-bit (ELF32) and 64-bit (ELF64) variants are supported, in either
little-endian or big-endian byte order.

The carver is **structure-based**: it parses the ELF header to compute the
file extent from the program-header table and section-header table offsets.

- Source: [src/carve/elf.rs](../../src/carve/elf.rs)
- Config validator: `elf`
- Output extensions: `elf`, `so`

## Signature Detection

**Header pattern**: `7F 45 4C 46` (`\x7FELF`)

All ELF files begin with this 4-byte magic number at offset `0`. The scanner
emits a hit on every occurrence of this signature in the evidence stream.

### Config Pattern

```yaml
- id: "elf"
  extensions: ["elf", "so"]
  header_patterns:
    - id: "elf_magic"
      hex: "7F454C46"
  footer_patterns: []
  max_size: 1073741824   # 1 GiB
  min_size: 52           # ELF32 header size
  validator: "elf"
```

## Carving Algorithm

ELF has no end-of-file marker. The carver instead derives the binary's extent
from the header tables described in `e_ident`/`Ehdr`:

1. **Read the first 64 bytes** at the candidate offset (covers both ELF32 and
   ELF64 headers).
2. **Validate `e_ident`**:
   - Magic bytes match `7F 45 4C 46`
   - `EI_CLASS` (`e_ident[4]`) is `1` (ELF32) or `2` (ELF64)
   - `EI_DATA` (`e_ident[5]`) is `1` (little-endian) or `2` (big-endian)
3. **Decode header fields** with the correct width and endianness:
   - `e_phoff`, `e_phentsize`, `e_phnum` — program-header table
   - `e_shoff`, `e_shentsize`, `e_shnum` — section-header table
4. **Compute the file extent** as the maximum of:
   - `e_phoff + (e_phentsize × e_phnum)`
   - `e_shoff + (e_shentsize × e_shnum)`
5. **Apply size clamps**:
   - Cap the end offset at `global_offset + max_size`
   - Discard the carve if `written < min_size`
6. **Stream and hash** the byte range `[global_offset, total_end)`,
   incrementally computing MD5/SHA-256 if enabled.

All multiplications use `saturating_mul` and additions use `saturating_add`
to prevent integer overflow on hostile or corrupt headers.

### State Diagram

```
START
  ↓
[Read 64-byte header at hit]
  ↓
[Validate magic, e_class, e_data]
  ↓
[Decode e_phoff/e_phnum/e_phentsize]
[Decode e_shoff/e_shnum/e_shentsize]
  ↓
size = max(phoff + phentsize*phnum,
           shoff + shentsize*shnum)
  ↓
end   = min(global_offset + size,
            global_offset + max_size)
  ↓
[Stream evidence into output, hash incrementally]
  ↓
  ├─ written < min_size → DISCARD
  ├─ EOF reached early  → TRUNCATED (kept)
  └─ Wrote full extent  → VALIDATED
```

## Validation

Pre-validation (`pre_validate`) rejects hits when:

- The first 7 bytes cannot be read (truncated)
- `e_ident[0..4]` ≠ `7F 45 4C 46`
- `EI_CLASS` is not `1` or `2`
- `EI_DATA` is not `1` or `2`
- `EI_VERSION` (`e_ident[6]`) ≠ `1`

Carve-time validation (`process_hit`) re-checks magic, class, and endianness,
and discards the carve if fewer than `min_size` bytes were written.

| Outcome | Meaning |
|---------|---------|
| `validated = true` | The full computed extent was written successfully |
| `truncated = true` | EOF reached before reaching `total_end` |
| Discarded | Header invalid or carve smaller than `min_size` |

The carver does **not** parse program/section content, so structural validity
beyond the table offsets is not asserted. A binary with a plausible header
but corrupt sections will still be carved. Downstream tools (e.g. `readelf`,
`file`) can be used for deeper validation.

## Size Constraints

- **Default `min_size`**: `52` bytes (the size of the ELF32 `Ehdr`)
- **Default `max_size`**: `1 GiB` (`1073741824`)
- Files smaller than `min_size` after writing are discarded
- Files whose computed extent exceeds `max_size` are truncated to `max_size`
- A computed extent of `0` (no PHT and no SHT) falls back to the 64-byte
  header length, so a bare header is still carved if it meets `min_size`

## Hash Computation

- **MD5** and **SHA-256** are computed incrementally as bytes are streamed
  to the output file (when enabled in `HashConfig`)
- Hashes cover only the carved range `[global_start, global_end]`
- Both hashes are emitted as lowercase hex strings in the metadata row

## Testing

**Test files**:
- Unit test: [src/carve/elf.rs](../../src/carve/elf.rs) (`carves_minimal_elf64`)
- Golden image: [tests/golden_image_test.rs](../../tests/golden_image_test.rs)

### Unit Test

`carves_minimal_elf64` builds an in-memory 128-byte ELF64 header with a
single program-header entry and a single section-header entry, runs it
through `process_hit`, and asserts that the carved size equals the input
length.

### Golden Image

The golden image manifest at
[tests/golden_image/manifest.json](../../tests/golden_image/manifest.json)
contains two ELF artefacts under `binaries/`:

- `test_elf` — small ELF64 executable
- `libtest.so` — small ELF64 shared object

Each entry asserts an exact `size` and `sha256`, so any regression in extent
calculation or hashing fails the run.

## Edge Cases

- **ELF32 vs ELF64**: `e_class` selects the header layout. ELF32 uses 32-bit
  table offsets at bytes 28/32; ELF64 uses 64-bit offsets at bytes 32/40.
- **Endianness**: All multi-byte fields are decoded according to `e_data`.
  Big-endian binaries (e.g. classic SPARC, PowerPC) are handled identically.
- **Stripped binaries**: The section-header table may be absent
  (`e_shoff == 0`). Extent is then derived from the program-header table
  alone.
- **Core dumps**: ETYPE = `ET_CORE`. Often very large; the `max_size` clamp
  protects the carver from runaway sizes.
- **Object files** (`.o`): Typically have no program-header table
  (`e_phoff == 0`). The section-header table covers the extent.
- **Tables beyond EOF**: If `e_phoff`/`e_shoff` point past the actual file
  end, the streaming write returns `eof_truncated = true` and the carve is
  marked `truncated`.
- **Overflowing fields**: All `*_offset + entsize × num` computations use
  saturating arithmetic, so malicious headers cannot produce a panic or wrap.
- **Embedded ELF**: ELF binaries embedded inside other files (e.g. `initrd`
  cpio archives, `.deb` packages) are detected and carved as standalone
  artefacts.

## Performance

- **Memory usage**: Constant. Only a 64-byte header buffer is held in
  addition to the streaming I/O buffer reused from `ExtractionContext`.
- **I/O pattern**: One small header read followed by a single sequential
  copy of the computed extent.
- **CPU**: Dominated by hash computation. The carver itself performs only
  a handful of integer operations per hit.
- **Marked `is_fast = true`**: The carver runs on the fast carving lane.

## Forensic Considerations

- **Build identifiers**: ELF binaries frequently contain a GNU build-id in
  the `.note.gnu.build-id` section, which is invaluable for matching against
  symbol servers and threat-intel feeds. SwiftBeaver does not extract the
  build-id today, but the carved file is byte-identical to the original and
  can be analysed with `readelf -n` post-carve.
- **Embedded paths and strings**: Library paths (`DT_RPATH`, `DT_RUNPATH`),
  interpreter (`PT_INTERP`), and symbol names commonly contain absolute
  filesystem paths that aid attribution.
- **Debug info**: `.debug_*` sections (DWARF) may include source paths, line
  numbers, and inlined function names.
- **Provenance**: Every carved row includes the standard provenance fields:
  - `run_id`
  - `tool_version`
  - `config_hash`
  - `evidence_path`
  - `evidence_sha256` (when available)
  - `pattern_id = "elf_magic"`
- **Evidence integrity**: Source evidence is opened read-only and never
  modified.
- **Reproducibility**: Carving the same evidence with the same config
  produces byte-identical output and identical hashes.

## Structure Examples

### ELF64 Header Layout (little-endian)

```
Offset  Size  Field           Notes
------  ----  --------------  ---------------------------
0x00     4    e_ident[0..4]   7F 45 4C 46  ("\x7FELF")
0x04     1    EI_CLASS        1 = ELF32, 2 = ELF64
0x05     1    EI_DATA         1 = LE,    2 = BE
0x06     1    EI_VERSION      1
0x07     9    e_ident[7..16]  OSABI / pad
0x10     2    e_type          ET_EXEC, ET_DYN, ET_CORE, ...
0x12     2    e_machine       EM_X86_64, EM_AARCH64, ...
0x14     4    e_version
0x18     8    e_entry
0x20     8    e_phoff         ← program-header table offset
0x28     8    e_shoff         ← section-header table offset
0x30     4    e_flags
0x34     2    e_ehsize        usually 64
0x36     2    e_phentsize     usually 56
0x38     2    e_phnum
0x3A     2    e_shentsize     usually 64
0x3C     2    e_shnum
0x3E     2    e_shstrndx
```

ELF32 uses the same field order but with 32-bit `e_entry`, `e_phoff`,
`e_shoff` (offsets at bytes 24, 28, 32 respectively) and a total header size
of 52 bytes.

### Extent Computation

```
extent = max(
    e_phoff + e_phentsize * e_phnum,
    e_shoff + e_shentsize * e_shnum,
)
```

## Known Limitations

- **No deep structural validation**: Program and section headers are not
  parsed beyond computing extents. A header with valid table offsets but
  corrupt entries will still be carved.
- **Trailing data ignored**: Some toolchains append data (e.g. signed code
  signatures, `objcopy --add-section` payloads beyond the SHT) past the
  computed extent. SwiftBeaver does not currently scan past the larger of
  PHT/SHT end. Such trailing data may be missed.
- **No build-id extraction**: GNU build-id and ELF notes are not surfaced
  as metadata fields (the carved file itself preserves them).
- **No symbol/DWARF parsing**: Symbol tables and debug info are kept in the
  carved bytes but not extracted into structured metadata.
- **Macho/PE not handled here**: Mach-O and PE/COFF executables use
  different magics and are out of scope for this carver.

## Related Carvers

- **PE/COFF** (Windows executables) — separate carver
- **Mach-O** (macOS executables) — not yet implemented
- **LNK** ([lnk.md](lnk.md)) — Windows shortcut files often reference ELF or
  PE binaries indirectly
- **Prefetch** ([prefetch.md](prefetch.md)) — Windows execution traces;
  conceptually parallel to ELF for Linux but no equivalent OS artefact
