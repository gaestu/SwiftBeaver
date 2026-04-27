#!/usr/bin/env python3
"""Generate synthetic Windows Registry hive (regf) samples for the golden image.

The output files are NOT real Microsoft hives. They are minimal, byte-deterministic
fixtures crafted to exercise the SwiftBeaver registry hive carver:

    * valid `regf` base block (4096 bytes) with correct XOR checksum
    * embedded UTF-16LE filename so hive-type heuristics work
    * one minimal 4096-byte `hbin` containing a single `nk` (named key) root cell
    * total file size: 8192 bytes (base block + 1 hbin)

Reference: Microsoft "Windows registry file format specification" / libregf docs.

Run from this directory:

    python3 generate.py

This script is deterministic: re-running it produces byte-identical files
(SHA-256 stable). Sample files are forensic fixtures with no PII.
"""

from __future__ import annotations

import struct
from pathlib import Path

OUT_DIR = Path(__file__).parent

# Fixed reference timestamp: 2024-01-01T00:00:00Z as Windows FILETIME
# (100 ns intervals since 1601-01-01).
FILETIME_2024_01_01 = 133485984000000000


def utf16le_fixed(text: str, total_bytes: int) -> bytes:
    """Encode `text` as UTF-16LE and zero-pad/truncate to `total_bytes`."""
    encoded = text.encode("utf-16-le")
    if len(encoded) > total_bytes:
        encoded = encoded[:total_bytes]
    return encoded + b"\x00" * (total_bytes - len(encoded))


def xor_checksum(header: bytes) -> int:
    """XOR of u32 little-endian words over the first 508 bytes of the base block."""
    assert len(header) >= 0x1FC
    acc = 0
    for i in range(0, 0x1FC, 4):
        acc ^= struct.unpack_from("<I", header, i)[0]
    return acc & 0xFFFFFFFF


def build_base_block(
    *,
    embedded_name: str,
    primary_seq: int = 1,
    secondary_seq: int = 1,
    timestamp: int = FILETIME_2024_01_01,
    major: int = 1,
    minor: int = 5,
    file_type: int = 0,
    file_format: int = 1,
    root_cell_offset: int = 0x20,
    hive_bins_data_size: int = 0x1000,
    clustering: int = 1,
) -> bytes:
    """Build a 4096-byte regf base block with a valid XOR checksum."""
    block = bytearray(4096)
    block[0x00:0x04] = b"regf"
    struct.pack_into("<I", block, 0x04, primary_seq)
    struct.pack_into("<I", block, 0x08, secondary_seq)
    struct.pack_into("<Q", block, 0x0C, timestamp)
    struct.pack_into("<I", block, 0x14, major)
    struct.pack_into("<I", block, 0x18, minor)
    struct.pack_into("<I", block, 0x1C, file_type)
    struct.pack_into("<I", block, 0x20, file_format)
    struct.pack_into("<I", block, 0x24, root_cell_offset)
    struct.pack_into("<I", block, 0x28, hive_bins_data_size)
    struct.pack_into("<I", block, 0x2C, clustering)
    # Embedded filename: 64 bytes UTF-16LE at 0x30
    block[0x30:0x70] = utf16le_fixed(embedded_name, 64)
    # Compute and write XOR checksum at 0x1FC
    checksum = xor_checksum(bytes(block))
    struct.pack_into("<I", block, 0x1FC, checksum)
    return bytes(block)


def build_root_nk_cell(name: str) -> bytes:
    """Build a minimal `nk` (named key) cell marked as root key.

    Layout (relative to cell start):
        +0x00  i32   cell_size (negative => allocated)
        +0x04  u16   "nk" signature
        +0x06  u16   flags (0x002C = root key + ASCII name)
        +0x08  u64   last_write FILETIME
        +0x10  u32   access_bits
        +0x14  u32   parent offset
        +0x18  u32   subkeys_count
        +0x1C  u32   volatile_subkeys_count
        +0x20  u32   subkeys_list_offset (0xFFFFFFFF = none)
        +0x24  u32   volatile_subkeys_list_offset
        +0x28  u32   values_count
        +0x2C  u32   values_list_offset
        +0x30  u32   security_key_offset
        +0x34  u32   class_name_offset
        +0x38  u32   largest_subkey_name_size
        +0x3C  u32   largest_subkey_class_size
        +0x40  u32   largest_value_name_size
        +0x44  u32   largest_value_data_size
        +0x48  u32   workvar
        +0x4C  u16   name_length
        +0x4E  u16   class_name_length
        +0x50  ...   name bytes (ASCII when 0x0020 flag is set)
    """
    name_bytes = name.encode("ascii")
    # Header is 0x50 bytes; pad cell to a multiple of 8.
    raw = bytearray(0x50)
    raw[0x00:0x02] = b"nk"
    struct.pack_into("<H", raw, 0x02, 0x002C)  # ROOT_KEY | ASCII name flags
    struct.pack_into("<Q", raw, 0x04, FILETIME_2024_01_01)
    struct.pack_into("<I", raw, 0x0C, 0)         # access bits
    struct.pack_into("<I", raw, 0x10, 0xFFFFFFFF)  # parent (none for root)
    struct.pack_into("<I", raw, 0x14, 0)         # subkeys_count
    struct.pack_into("<I", raw, 0x18, 0)
    struct.pack_into("<I", raw, 0x1C, 0xFFFFFFFF)
    struct.pack_into("<I", raw, 0x20, 0xFFFFFFFF)
    struct.pack_into("<I", raw, 0x24, 0)         # values_count
    struct.pack_into("<I", raw, 0x28, 0xFFFFFFFF)
    struct.pack_into("<I", raw, 0x2C, 0xFFFFFFFF)  # security_key_offset
    struct.pack_into("<I", raw, 0x30, 0xFFFFFFFF)  # class_name_offset
    struct.pack_into("<I", raw, 0x34, 0)
    struct.pack_into("<I", raw, 0x38, 0)
    struct.pack_into("<I", raw, 0x3C, 0)
    struct.pack_into("<I", raw, 0x40, 0)
    struct.pack_into("<I", raw, 0x44, 0)
    struct.pack_into("<H", raw, 0x48, name_length := len(name_bytes))
    struct.pack_into("<H", raw, 0x4A, 0)         # class_name_length

    body = bytes(raw) + name_bytes
    # 4-byte cell-size header; pad whole cell to 8 bytes.
    total = 4 + len(body)
    pad = (-total) % 8
    cell_size = total + pad
    cell = struct.pack("<i", -cell_size) + body + b"\x00" * pad
    return cell


def build_hbin(root_name: str, *, hbin_offset: int = 0, hbin_size: int = 0x1000) -> bytes:
    """Build a 4096-byte hbin containing a single root nk cell."""
    bin_buf = bytearray(hbin_size)
    bin_buf[0x00:0x04] = b"hbin"
    struct.pack_into("<I", bin_buf, 0x04, hbin_offset)
    struct.pack_into("<I", bin_buf, 0x08, hbin_size)
    struct.pack_into("<Q", bin_buf, 0x0C, 0)  # unknown1/2
    struct.pack_into("<Q", bin_buf, 0x14, FILETIME_2024_01_01)
    struct.pack_into("<I", bin_buf, 0x1C, 0)  # spare

    # First cell starts at offset 0x20 within the hbin.
    nk = build_root_nk_cell(root_name)
    bin_buf[0x20:0x20 + len(nk)] = nk
    # Remainder is free space; mark as one free cell with positive size.
    free_off = 0x20 + len(nk)
    free_size = hbin_size - free_off
    if free_size >= 4:
        struct.pack_into("<i", bin_buf, free_off, free_size)
    return bytes(bin_buf)


def write_hive(
    filename: str,
    *,
    embedded_name: str,
    root_name: str,
    primary_seq: int = 1,
    secondary_seq: int = 1,
) -> None:
    base = build_base_block(
        embedded_name=embedded_name,
        primary_seq=primary_seq,
        secondary_seq=secondary_seq,
    )
    hbin = build_hbin(root_name)
    out = OUT_DIR / filename
    out.write_bytes(base + hbin)
    print(f"  wrote {out.relative_to(OUT_DIR.parents[3])}  ({len(base) + len(hbin)} bytes)")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)

    # Embedded filename inside the regf header is normally a path like
    # `\??\C:\Windows\System32\config\SYSTEM`. The hive-type heuristic in
    # the carver matches the trailing component.
    # The embedded filename field is 64 bytes (32 UTF-16LE chars).
    # Real Windows hives commonly store either a short name (e.g. "SYSTEM")
    # or a truncated path. We use short names so hive-type heuristics in
    # the carver have an unambiguous trailing component to match on.
    write_hive(
        "NTUSER.DAT.synthetic",
        embedded_name="NTUSER.DAT",
        root_name="CMI-CreateHive{NTUSER}",
    )
    write_hive(
        "SYSTEM.synthetic",
        embedded_name="SYSTEM",
        root_name="CMI-CreateHive{SYSTEM}",
    )
    write_hive(
        "SOFTWARE.synthetic",
        embedded_name="SOFTWARE",
        root_name="CMI-CreateHive{SOFTWARE}",
    )
    write_hive(
        "SAM.synthetic",
        embedded_name="SAM",
        root_name="CMI-CreateHive{SAM}",
    )
    # Dirty hive: primary != secondary sequence number.
    # Embedded name still says SYSTEM so type detection still works.
    write_hive(
        "DIRTY.synthetic",
        embedded_name="SYSTEM",
        root_name="CMI-CreateHive{DIRTY}",
        primary_seq=7,
        secondary_seq=6,
    )


if __name__ == "__main__":
    main()
