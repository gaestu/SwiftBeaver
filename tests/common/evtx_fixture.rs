//! Synthetic EVTX fixture builder for tests.
//!
//! Mirrors `tests/golden_image/samples/windows/evtx/generate.py` but in
//! Rust so tests don't depend on Python at runtime. The CRC32 fields are
//! left zero — the carver does not perform CRC validation (carve-the-bytes
//! semantics), and synthetic chunks set `last_record = first_record - 1`
//! to indicate "empty records area".

const HEADER_SIZE: usize = 4096;
const CHUNK_SIZE: usize = 65536;
const INNER_HEADER_SIZE: u32 = 0x80;
const HEADER_BLOCK_SIZE: u16 = 4096;
const MAJOR_VERSION: u16 = 3;

/// Build a single 64 KiB EVTX chunk header.
///
/// `first_record` and `last_record` are the chunk's first/last record *numbers*.
/// For an empty chunk, set `last_record = first_record - 1` (or `last_record < first_record`).
pub fn build_chunk(first_record: u64, last_record: u64) -> Vec<u8> {
    let mut chunk = vec![0u8; CHUNK_SIZE];
    chunk[0..8].copy_from_slice(b"ElfChnk\x00");
    chunk[0x08..0x10].copy_from_slice(&first_record.to_le_bytes());
    chunk[0x10..0x18].copy_from_slice(&last_record.to_le_bytes());
    // first/last record identifiers (mirror the numbers for fixtures)
    chunk[0x18..0x20].copy_from_slice(&first_record.to_le_bytes());
    chunk[0x20..0x28].copy_from_slice(&last_record.to_le_bytes());
    chunk[0x28..0x2C].copy_from_slice(&INNER_HEADER_SIZE.to_le_bytes());
    chunk
}

/// Build the 4096-byte EVTX file header.
pub fn build_file_header(
    first_chunk: u64,
    last_chunk: u64,
    chunk_count: u16,
    minor_version: u16,
    file_flags: u32,
) -> Vec<u8> {
    let mut h = vec![0u8; HEADER_SIZE];
    h[0..8].copy_from_slice(b"ElfFile\x00");
    h[0x08..0x10].copy_from_slice(&first_chunk.to_le_bytes());
    h[0x10..0x18].copy_from_slice(&last_chunk.to_le_bytes());
    // NextRecordIdentifier — value irrelevant for the carver (we walk chunks).
    h[0x18..0x20].copy_from_slice(&1u64.to_le_bytes());
    h[0x20..0x24].copy_from_slice(&INNER_HEADER_SIZE.to_le_bytes());
    h[0x24..0x26].copy_from_slice(&minor_version.to_le_bytes());
    h[0x26..0x28].copy_from_slice(&MAJOR_VERSION.to_le_bytes());
    h[0x28..0x2A].copy_from_slice(&HEADER_BLOCK_SIZE.to_le_bytes());
    h[0x2A..0x2C].copy_from_slice(&chunk_count.to_le_bytes());
    h[0x78..0x7C].copy_from_slice(&file_flags.to_le_bytes());
    h
}

pub struct EvtxBuildSpec {
    pub chunk_count: u16,
    /// Records per chunk. Length must equal `chunk_count`. Use 0 for an empty chunk.
    pub records_per_chunk: Vec<u64>,
    pub minor_version: u16,
    pub file_flags: u32,
}

/// Assemble a complete synthetic EVTX file according to the spec.
pub fn build_evtx(spec: &EvtxBuildSpec) -> Vec<u8> {
    assert!(spec.chunk_count >= 1, "chunk_count must be >= 1");
    assert_eq!(
        spec.records_per_chunk.len(),
        spec.chunk_count as usize,
        "records_per_chunk length must match chunk_count"
    );
    let mut out = Vec::with_capacity(HEADER_SIZE + CHUNK_SIZE * spec.chunk_count as usize);
    out.extend(build_file_header(
        0,
        u64::from(spec.chunk_count - 1),
        spec.chunk_count,
        spec.minor_version,
        spec.file_flags,
    ));
    let mut next_record_number: u64 = 1;
    for &count in &spec.records_per_chunk {
        if count == 0 {
            // Empty: last < first sentinel.
            out.extend(build_chunk(next_record_number, next_record_number - 1));
        } else {
            let last = next_record_number + count - 1;
            out.extend(build_chunk(next_record_number, last));
            next_record_number = last + 1;
        }
    }
    out
}
