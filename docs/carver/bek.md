# BitLocker BEK Carver

## Overview

The BitLocker BEK carver extracts binary BitLocker External Key (`.bek`) startup/recovery key files. These are distinct from textual 48-digit BitLocker recovery passwords and from BitLocker key package (`.KPG`) files. SwiftBeaver only carves and records metadata for the BEK artefact; it does not unlock or decrypt BitLocker volumes.

## Signature Detection

BEK files do not have a strong standalone magic value. The default scanner looks for the version/header-size field sequence at offset `+4`:

```text
01 00 00 00 30 00 00 00
```

The carver subtracts four bytes from that hit and accepts the candidate only after full structural validation.

## Carving Algorithm

1. Read the 48-byte BEK metadata header.
2. Require version `1`, header size `48`, `metadata_size >= 48`, matching metadata-size copy, and a bounded total size.
3. Parse FVE metadata entries with strict entry-size and bounds checks.
4. Require a startup-key entry (`entry_type = 0x0006`) with external-key value (`value_type = 0x0009`).
5. Parse the external-key body: 16-byte key identifier GUID, 8-byte FILETIME, then nested metadata-entry properties.
6. Require a nested key property (`value_type = 0x0001`) with a 4-byte key/encryption method followed by exactly 32 key bytes.
7. Optionally decode a nested UTF-16LE description property (`value_type = 0x0002`).
8. Copy exactly the validated metadata size into the configured output directory.

## Metadata

Every valid BEK is recorded in `carved_files` like other carved files. A second BEK-specific metadata row is written to:

- JSONL: `metadata/bitlocker_bek.jsonl`
- CSV: `metadata/bitlocker_bek.csv`
- Parquet: `parquet/artefacts_bitlocker_bek.parquet`

The BEK row includes provenance fields plus `global_start`, `global_end`, `size`, `carved_path`, `key_identifier_guid`, optional `description`, `key_data_length`, `key_encryption_method`, and `modification_filetime`.

## Size Constraints

The default maximum size is 64 KiB, and the carver enforces 64 KiB as a hard upper bound even if configuration sets a larger `max_size`. The common BEK size is much smaller, but the larger bounded cap allows conservative support for metadata padding or uncommon property layouts while preventing runaway carving.

## Forensic Considerations

- Evidence is read-only; carved bytes are written only under the configured output directory.
- Detection is structural, not filename- or extension-based.
- Startup key bytes are preserved in the carved BEK file. Metadata records only the key data length and method, not the key bytes themselves.
- The carver does not parse `.KPG` files or textual BitLocker recovery passwords.