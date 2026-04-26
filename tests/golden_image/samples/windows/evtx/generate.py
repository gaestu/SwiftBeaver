#!/usr/bin/env python3
"""Generate synthetic Windows EVTX samples for the golden image.

Output files are NOT real Microsoft event logs. They are minimal,
byte-deterministic fixtures that exercise the SwiftBeaver EVTX carver:

    * valid `ElfFile\\x00` file header (4096 bytes) with correct CRC32
    * one or more empty `ElfChnk\\x00` chunks (65536 bytes each) with valid
      header + empty event-records CRC32
    * deterministic FILETIME, sequence numbers and chunk counts

Reference: libyal/libevtx "Windows XML Event Log (EVTX) format" specification.

Run from this directory:

    python3 generate.py

Re-running produces byte-identical files (SHA-256 stable). These fixtures
contain no PII or real event data.
"""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

OUT_DIR = Path(__file__).parent

HEADER_SIZE = 4096
CHUNK_SIZE = 65536
RECORDS_OFFSET = 0x200  # first event record offset within a chunk

# Fixed reference timestamp: 2024-01-01T00:00:00Z as Windows FILETIME
# (100 ns intervals since 1601-01-01).
FILETIME_2024_01_01 = 133485984000000000

# EVTX format version emitted by Windows 7+.
MAJOR_VERSION = 3
MINOR_VERSION = 1
HEADER_BLOCK_SIZE = 4096
INNER_HEADER_SIZE = 0x80


def build_chunk(
    *,
    chunk_number: int,
    first_record_number: int,
    last_record_number: int,
    first_record_id: int,
    last_record_id: int,
) -> bytes:
    """Build a 64 KiB EVTX chunk with empty event-records area."""
    chunk = bytearray(CHUNK_SIZE)
    chunk[0:8] = b"ElfChnk\x00"
    struct.pack_into("<Q", chunk, 0x08, first_record_number)
    struct.pack_into("<Q", chunk, 0x10, last_record_number)
    struct.pack_into("<Q", chunk, 0x18, first_record_id)
    struct.pack_into("<Q", chunk, 0x20, last_record_id)
    struct.pack_into("<I", chunk, 0x28, INNER_HEADER_SIZE)
    # LastEventRecordDataOffset: 0 means no records present.
    struct.pack_into("<I", chunk, 0x2C, 0)
    # FreeSpaceOffset: first byte after records area (== records start when empty).
    struct.pack_into("<I", chunk, 0x30, RECORDS_OFFSET)
    # EventRecordsChecksum: CRC32 of bytes [0x200 .. FreeSpaceOffset). Empty -> 0.
    struct.pack_into("<I", chunk, 0x34, 0)
    # Bytes 0x38..0x78 are reserved/unused; leave as zero.
    # 0x78..0x7C is reserved (often zero on synthetic chunks).
    # Header checksum: CRC32 of bytes [0x00..0x78) + [0x80..0x200).
    crc_input = bytes(chunk[0x00:0x78]) + bytes(chunk[0x80:0x200])
    header_crc = zlib.crc32(crc_input) & 0xFFFFFFFF
    struct.pack_into("<I", chunk, 0x7C, header_crc)

    # Discriminator so synthetic chunks are visually distinguishable in hex
    # dumps without affecting checksums (the trailing area is unused).
    marker = f"SwiftBeaver synthetic EVTX chunk #{chunk_number}\x00".encode("utf-8")
    chunk[CHUNK_SIZE - len(marker):] = marker
    return bytes(chunk)


def build_file_header(
    *,
    first_chunk: int,
    last_chunk: int,
    next_record_id: int,
    chunk_count: int,
    file_flags: int = 0,  # 0 = clean, 1 = dirty, 2 = full
) -> bytes:
    """Build the 4096-byte EVTX file header with valid CRC32 checksum."""
    header = bytearray(HEADER_SIZE)
    header[0:8] = b"ElfFile\x00"
    struct.pack_into("<Q", header, 0x08, first_chunk)
    struct.pack_into("<Q", header, 0x10, last_chunk)
    struct.pack_into("<Q", header, 0x18, next_record_id)
    struct.pack_into("<I", header, 0x20, INNER_HEADER_SIZE)
    struct.pack_into("<H", header, 0x24, MINOR_VERSION)
    struct.pack_into("<H", header, 0x26, MAJOR_VERSION)
    struct.pack_into("<H", header, 0x28, HEADER_BLOCK_SIZE)
    struct.pack_into("<H", header, 0x2A, chunk_count)
    # 0x2C..0x78 reserved (zero).
    struct.pack_into("<I", header, 0x78, file_flags)
    # File header checksum: CRC32 of first 120 bytes.
    file_crc = zlib.crc32(bytes(header[0:0x78])) & 0xFFFFFFFF
    struct.pack_into("<I", header, 0x7C, file_crc)
    return bytes(header)


def build_evtx(
    *,
    chunk_count: int,
    file_flags: int = 0,
    first_record_id: int = 1,
) -> bytes:
    """Assemble a complete synthetic EVTX file with `chunk_count` empty chunks."""
    if chunk_count < 1:
        raise ValueError("chunk_count must be >= 1")

    parts = [
        build_file_header(
            first_chunk=0,
            last_chunk=chunk_count - 1,
            # NextRecordIdentifier = id of the next record that *would* be written.
            next_record_id=first_record_id,
            chunk_count=chunk_count,
            file_flags=file_flags,
        )
    ]
    for i in range(chunk_count):
        parts.append(
            build_chunk(
                chunk_number=i,
                first_record_number=first_record_id,
                last_record_number=first_record_id - 1,  # empty -> last < first
                first_record_id=first_record_id,
                last_record_id=first_record_id - 1,
            )
        )
    return b"".join(parts)


def write(path: Path, data: bytes) -> None:
    path.write_bytes(data)
    print(f"  wrote {path.name:40s} {len(data):>8d} bytes")


def main() -> None:
    # Single-chunk clean log (smallest valid synthetic EVTX: 4 KiB + 64 KiB).
    write(
        OUT_DIR / "Application.synthetic.evtx",
        build_evtx(chunk_count=1, file_flags=0),
    )
    # Two-chunk clean log to exercise multi-chunk size calculation.
    write(
        OUT_DIR / "System.synthetic.evtx",
        build_evtx(chunk_count=2, file_flags=0),
    )
    # Dirty flag set (graceful-shutdown indicator missing).
    write(
        OUT_DIR / "Security.synthetic.dirty.evtx",
        build_evtx(chunk_count=1, file_flags=1),
    )


if __name__ == "__main__":
    main()
