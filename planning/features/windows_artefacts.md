# Windows Artefacts Carving & Parsing

**Status:** WIP  
**Priority:** Medium  
**Effort:** Medium  

---

## Problem Statement

Windows systems contain numerous forensic artefacts that provide critical evidence about system activity, user behavior, and program execution. Currently, `SwiftBeaver` focuses on generic file types but lacks support for Windows-specific artefacts that are essential for incident response and forensic investigations.

Key Windows artefacts include:
- **LNK files** — Shortcut files that reveal accessed files, network paths, and timestamps
- **Prefetch files** — Execution history showing what programs ran and when
- **EVTX files** — Windows Event Logs containing security, system, and application events
- **Registry hives** — SAM, SYSTEM, SOFTWARE, NTUSER.DAT containing configuration and user data
- **JumpLists** — Recent/frequent files accessed per application

---

## Scope

### In Scope

1. **LNK (Shell Link) Files:**
   - Carve by header signature
   - Parse target path, timestamps, volume info
   - Extract to metadata

2. **Prefetch Files:**
   - Carve Windows XP/Vista/7/8/10/11 Prefetch formats
   - Parse executable name, run count, last run times
   - Handle compressed (Win10+) and uncompressed formats

3. **EVTX (Event Log) Files:**
   - Carve by header signature
   - Basic structure validation
   - Record event metadata (not full parsing)

4. **Registry Hives:**
   - Carve by "regf" signature
   - Basic validation
   - Identify hive type (SAM, SYSTEM, etc.) if possible

5. **Metadata output:**
   - New metadata category: `windows_artefacts.jsonl/csv/parquet/sqlite`
   - Structured records for each artefact type

### Out of Scope (Future Work)

- Full registry key/value parsing (complex, separate feature)
- Full EVTX event parsing (complex XML structure)
- JumpLists (require OLE compound document + custom format)
- $MFT parsing (filesystem-level, different scope)
- Memory artifacts (hiberfil.sys, pagefile.sys)

---

## Design Notes

### LNK File Format

**Signature:** `4C 00 00 00 01 14 02 00` (first 8 bytes)

**Structure:**
```
ShellLinkHeader (76 bytes)
├── HeaderSize (4 bytes) = 0x4C
├── LinkCLSID (16 bytes) = 00021401-0000-0000-C000-000000000046
├── LinkFlags (4 bytes)
├── FileAttributes (4 bytes)
├── CreationTime (8 bytes)
├── AccessTime (8 bytes)
├── WriteTime (8 bytes)
├── FileSize (4 bytes)
├── IconIndex (4 bytes)
├── ShowCommand (4 bytes)
├── HotKey (2 bytes)
├── Reserved (10 bytes)

LinkTargetIDList (optional, if HasLinkTargetIDList)
LinkInfo (optional, if HasLinkInfo)
StringData (optional)
ExtraData (optional)
```

**Parsed output:**
```rust
struct LnkArtefact {
    offset: u64,
    size: u64,
    target_path: Option<String>,
    working_dir: Option<String>,
    creation_time: Option<DateTime>,
    access_time: Option<DateTime>,
    write_time: Option<DateTime>,
    file_size: u32,
    volume_serial: Option<String>,
    local_base_path: Option<String>,
    network_path: Option<String>,
}
```

### Prefetch File Format

**Signature:** 
- Uncompressed: `SCCA` at offset 4 (versions 17, 23, 26)
- Compressed (Win10+): `MAM\x04` at offset 0

**Structure (varies by version):**
```
Header
├── Version (4 bytes)
├── Signature "SCCA" (4 bytes)
├── FileSize (4 bytes)
├── ExecutableName (60 bytes, UTF-16)
├── PrefetchHash (4 bytes)
├── ... (version-specific)

FileMetrics (array)
├── StartTime
├── Duration
├── ... 

VolumeInfo
├── VolumePath
├── CreationTime
├── SerialNumber

FileReferences (array)
├── FilePath strings
```

**Parsed output:**
```rust
struct PrefetchArtefact {
    offset: u64,
    size: u64,
    executable_name: String,
    prefetch_hash: String,
    run_count: u32,
    last_run_times: Vec<DateTime>,  // Up to 8 timestamps
    volume_paths: Vec<String>,
    referenced_files: Vec<String>,  // Optional, can be large
    version: u8,  // 17=XP, 23=Vista/7, 26=8.x, 30=10/11
}
```

### EVTX File Format

**Signature:** `ElfFile\x00` (8 bytes)

**Structure:**
```
FileHeader (4096 bytes)
├── Signature "ElfFile\x00"
├── FirstChunkNumber
├── LastChunkNumber
├── NextRecordID
├── HeaderSize
├── ...

Chunks (65536 bytes each)
├── ChunkHeader
├── Records (BinXML format)
```

**Parsed output (basic):**
```rust
struct EvtxArtefact {
    offset: u64,
    size: u64,
    first_chunk: u64,
    last_chunk: u64,
    record_count_estimate: u64,
    log_name: Option<String>,  // If determinable from path/content
}
```

### Registry Hive Format

**Signature:** `regf` (4 bytes)

**Structure:**
```
Base Block (4096 bytes)
├── Signature "regf"
├── Sequence1
├── Sequence2
├── Timestamp
├── Major/Minor version
├── Type (0=normal)
├── RootCellOffset
├── HiveLength
├── FileName (optional, UTF-16)

Hive Bins
├── Bin Header "hbin"
├── Cells (keys, values, etc.)
```

**Parsed output:**
```rust
struct RegistryHiveArtefact {
    offset: u64,
    size: u64,
    timestamp: Option<DateTime>,
    hive_name: Option<String>,  // From embedded filename
    hive_type: Option<String>,  // SAM, SYSTEM, SOFTWARE, etc.
    root_key_name: Option<String>,
}
```

---

## Implementation Plan

### Phase 1: Infrastructure

1. **Create `src/carve/windows/` module directory:**
   ```
   src/carve/windows/
   ├── mod.rs
   ├── lnk.rs
   ├── prefetch.rs
   ├── evtx.rs
   └── registry.rs
   ```

2. **Create metadata records:**
   ```rust
   // src/metadata/records.rs or similar
   pub enum WindowsArtefactRecord {
       Lnk(LnkArtefact),
       Prefetch(PrefetchArtefact),
       Evtx(EvtxArtefact),
       RegistryHive(RegistryHiveArtefact),
   }
   ```

3. **Update metadata sinks:**
   - Add `record_windows_artefact()` method to `MetadataSink`
   - Implement for JSONL, CSV, Parquet, SQLite

### Phase 2: LNK Carver

4. **Implement `src/carve/windows/lnk.rs`:**
   - Header signature detection
   - Size determination (parse LinkFlags to find sections)
   - Target path extraction
   - Timestamp parsing

5. **Add to config:**
   ```yaml
   - id: "lnk"
     extensions: ["lnk"]
     header_patterns:
       - id: "lnk_header"
         hex: "4C000000011402000000000000C0000000000000460"
     max_size: 10485760  # 10 MB
     min_size: 76
     validator: "lnk"
   ```

6. **Register in CarveRegistry**

### Phase 3: Prefetch Carver

7. **Implement `src/carve/windows/prefetch.rs`:**
   - Detect compressed (MAM) vs uncompressed (SCCA)
   - Handle decompression for Win10+ format
   - Parse version-specific structures
   - Extract run count and timestamps

8. **Add to config:**
   ```yaml
   - id: "prefetch"
     extensions: ["pf"]
     header_patterns:
       - id: "prefetch_mam"
         hex: "4D414D04"
       - id: "prefetch_scca_17"
         hex: "11000000534343410"
       - id: "prefetch_scca_23"
         hex: "17000000534343410"
       - id: "prefetch_scca_26"
         hex: "1A000000534343410"
       - id: "prefetch_scca_30"
         hex: "1E000000534343410"
     max_size: 10485760
     min_size: 84
     validator: "prefetch"
   ```

### Phase 4: EVTX Carver

9. **Implement `src/carve/windows/evtx.rs`:**
   - Header signature detection
   - Parse file header for chunk count
   - Calculate size from header or chunk parsing
   - Basic validation (not full event parsing)

10. **Add to config:**
    ```yaml
    - id: "evtx"
      extensions: ["evtx"]
      header_patterns:
        - id: "evtx_header"
          hex: "456C6646696C6500"
      max_size: 1073741824  # 1 GB
      min_size: 4096
      validator: "evtx"
    ```

### Phase 5: Registry Hive Carver

11. **Implement `src/carve/windows/registry.rs`:**
    - "regf" signature detection
    - Parse base block for size and metadata
    - Extract embedded filename
    - Identify hive type heuristically

12. **Add to config:**
    ```yaml
    - id: "registry"
      extensions: ["reg"]
      header_patterns:
        - id: "regf_header"
          hex: "72656766"
      max_size: 536870912  # 512 MB
      min_size: 4096
      validator: "registry"
    ```

### Phase 6: Testing

13. **Create test files:**
    - `tests/carver_lnk.rs`
    - `tests/carver_prefetch.rs`
    - `tests/carver_evtx.rs`
    - `tests/carver_registry.rs`

14. **Test scenarios:**
    - Valid file carving
    - Truncated file handling
    - Metadata extraction accuracy
    - Size limit enforcement

### Phase 7: Documentation

15. **Create `docs/windows_artefacts.md`:**
    - Supported artefact types
    - Metadata schema for each type
    - Known limitations

16. **Update README.md:**
    - Add Windows artefacts to feature list
    - Add CLI examples

---

## Expected Tests

### LNK Tests
- `test_lnk_basic_carve` — carve valid LNK file
- `test_lnk_target_path_extraction` — verify target path parsed
- `test_lnk_timestamps` — verify timestamps parsed correctly
- `test_lnk_network_path` — handle network share paths

### Prefetch Tests
- `test_prefetch_win10_compressed` — carve compressed Win10 prefetch
- `test_prefetch_win7_uncompressed` — carve uncompressed Win7 prefetch
- `test_prefetch_executable_name` — verify exe name extracted
- `test_prefetch_run_count` — verify run count parsed

### EVTX Tests
- `test_evtx_basic_carve` — carve valid EVTX file
- `test_evtx_chunk_count` — verify chunk count from header
- `test_evtx_truncated` — handle truncated gracefully

### Registry Tests
- `test_registry_basic_carve` — carve valid registry hive
- `test_registry_hive_type_detection` — identify SAM/SYSTEM/etc.
- `test_registry_embedded_filename` — extract filename

---

## Impact on Docs and README

- **README.md:**
  - Add LNK/Prefetch/EVTX/Registry to carved file types
  - Add Windows forensics section
  - Add example: `--types lnk,prefetch,evtx,registry`

- **docs/windows_artefacts.md:** New comprehensive documentation

- **docs/metadata_jsonl.md:** Add `windows_artefacts` schema

- **docs/metadata_parquet.md:** Add Windows artefact columns

---

## Dependencies

Consider adding:
- `compress` or `lzxpress` crate for Win10 Prefetch decompression
- Or implement LZXPRESS decompression (it's relatively simple)

---

## Open Questions

1. Should we parse Prefetch referenced files list? (Can be hundreds of entries)
2. Should EVTX parsing go deeper (parse BinXML records)?
3. Should Registry parsing extract any key/value data?
4. How to handle carved artefacts that are also valid regular files? (e.g., LNK is also a regular file)
5. Should we add a `--windows-artefacts` convenience flag to enable all Windows types?
