use crate::carve::windows::WindowsArtefactRecord;
use crate::carve::{
    CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext, PendingCarve,
    PostCarveMetadata, PreValidation, output_path,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

// ------------------------------------------------------------------
// Layout constants — EVTX file/chunk format (libyal "evtx" spec)
// ------------------------------------------------------------------

/// EVTX file header is exactly 4096 bytes.
const FILE_HEADER_SIZE: u64 = 4096;
/// Each EVTX chunk is exactly 65536 bytes.
const CHUNK_SIZE: u64 = 65536;

const FILE_MAGIC: &[u8; 8] = b"ElfFile\x00";
const CHUNK_MAGIC: &[u8; 8] = b"ElfChnk\x00";

// Offsets within the file header.
const FH_FIRST_CHUNK_OFFSET: usize = 0x08; // u64 LE
const FH_LAST_CHUNK_OFFSET: usize = 0x10; // u64 LE
const FH_HEADER_BLOCK_SIZE_OFFSET: usize = 0x28; // u16 LE
#[cfg(test)]
const FH_MINOR_VERSION_OFFSET: usize = 0x24; // u16 LE
const FH_MAJOR_VERSION_OFFSET: usize = 0x26; // u16 LE
const FH_CHUNK_COUNT_OFFSET: usize = 0x2A; // u16 LE

// Offsets within a chunk header.
const CH_FIRST_RECORD_NUMBER_OFFSET: usize = 0x08; // u64 LE
const CH_LAST_RECORD_NUMBER_OFFSET: usize = 0x10; // u64 LE

/// Only EVTX format major version emitted by Windows (Vista+).
const SUPPORTED_MAJOR_VERSION: u16 = 3;
/// Defensive plausibility cap on declared chunk count.
/// 16383 chunks * 64 KiB + 4 KiB header = 1_073_680_384 bytes, just under the
/// default `max_size` of 1 GiB so the cap itself is reachable rather than
/// being silently rejected by the size gate.
const MAX_PLAUSIBLE_CHUNK_COUNT: u32 = 16_383;

// ------------------------------------------------------------------
// Public artefact type
// ------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct EvtxArtefact {
    pub run_id: String,
    pub offset: u64,
    pub size: u64,
    /// `FirstChunkNumber` as declared by the source file header. When
    /// `validated == false && truncated == true` on the corresponding
    /// `carved_files` row, this value reflects the source header and may
    /// reference a chunk index past the actually-carved extent.
    pub first_chunk: u64,
    /// `LastChunkNumber` as declared by the source file header. Same
    /// truncation caveat as `first_chunk`: callers indexing into the
    /// carved file by `last_chunk * 65536 + 4096` must check `size`
    /// (and `truncated`) first.
    pub last_chunk: u64,
    pub record_count_estimate: u64,
    pub log_name: Option<String>,
}

// ------------------------------------------------------------------
// CarveHandler implementation
// ------------------------------------------------------------------

pub struct EvtxCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl EvtxCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for EvtxCarveHandler {
    fn file_type(&self) -> &str {
        "evtx"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        // Verify both the file magic at offset 0 AND the ElfChnk\0 magic at
        // offset 0x1000 (start of chunk 0). The two-stage magic check kills
        // false-positive `ElfFile\0` byte sequences in compressed/random
        // evidence before they trigger up-to-1 GiB of speculative carve I/O.
        let mut head = [0u8; FILE_MAGIC.len()];
        if !fill_at(evidence, offset, &mut head)? {
            return Ok(PreValidation::Reject("truncated EVTX header".to_string()));
        }
        if &head != FILE_MAGIC {
            return Ok(PreValidation::Reject("EVTX magic mismatch".to_string()));
        }
        let chunk_offset = offset
            .checked_add(FILE_HEADER_SIZE)
            .ok_or_else(|| CarveError::Invalid("evtx chunk offset overflows u64".into()))?;
        let mut chunk_magic = [0u8; CHUNK_MAGIC.len()];
        if !fill_at(evidence, chunk_offset, &mut chunk_magic)? || &chunk_magic != CHUNK_MAGIC {
            return Ok(PreValidation::Reject(
                "EVTX first chunk magic missing".to_string(),
            ));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;

        // Read and validate the file header.
        let header_bytes = match read_file_header(ctx.evidence, hit.global_offset)? {
            Some(bytes) => bytes,
            None => {
                tracing::debug!(
                    offset = hit.global_offset,
                    "evtx hit rejected: truncated or invalid file header"
                );
                return Ok(None);
            }
        };
        let parsed = match parse_file_header(&header_bytes) {
            Some(h) => h,
            None => {
                tracing::debug!(
                    offset = hit.global_offset,
                    "evtx hit rejected: invalid file header fields"
                );
                return Ok(None);
            }
        };

        // Compute total size by walking the header-declared chunks. Only
        // verified contiguous `ElfChnk\0` chunks extend the carve range; a
        // corrupt or missing declared chunk stops the walk so adjacent evidence
        // is never folded into the EVTX output.
        let walked = walk_chunks(ctx.evidence, hit.global_offset, parsed.chunk_count)?;

        // Sizing uses the verified contiguous chunk count.
        let chunk_bytes = u64::from(walked.verified_chunks)
            .checked_mul(CHUNK_SIZE)
            .ok_or_else(|| CarveError::Invalid("evtx chunk size overflows u64".into()))?;
        let total_size = FILE_HEADER_SIZE
            .checked_add(chunk_bytes)
            .ok_or_else(|| CarveError::Invalid("evtx size overflows u64".into()))?;

        let max_size = self.max_size;
        if total_size > max_size {
            tracing::debug!(
                offset = hit.global_offset,
                total_size,
                max_size,
                "evtx hit rejected: total_size exceeds configured max_size"
            );
            return Ok(None);
        }
        if total_size < self.min_size {
            tracing::debug!(
                offset = hit.global_offset,
                total_size,
                min_size = self.min_size,
                "evtx hit rejected: total_size below configured min_size"
            );
            return Ok(None);
        }

        let record_count_estimate = walked.record_count_estimate;

        // Stream the full record (header + chunks) to disk while hashing in
        // a single pass.

        let mut stream = CarveStream::new(ctx, hit.global_offset, max_size, full_path.clone());

        let mut truncated = walked.missing_declared_chunks > 0;
        let mut errors: Vec<String> = Vec::new();

        if walked.corrupt_declared_chunks > 0 {
            errors.push(format!(
                "evtx stopped before {} declared chunk(s) with bad ElfChnk magic",
                walked.corrupt_declared_chunks
            ));
        }
        if walked.missing_declared_chunks > 0 {
            errors.push(format!(
                "evtx missing {} declared chunk(s) at evidence boundary",
                walked.missing_declared_chunks
            ));
        }

        match stream.consume_remaining(total_size) {
            Ok(()) => {}
            Err(CarveError::Truncated) | Err(CarveError::Eof) => {
                truncated = true;
                errors.push("evtx truncated by evidence boundary".to_string());
            }
            Err(other) => {
                stream.discard();
                return Err(other);
            }
        }

        let (size, md5_hex, sha256_hex, mut writer) = stream.finalize()?;

        // Drop sub-min-size partial carves rather than emitting unusable
        // evidence (only triggers in the truncation case).
        if size < self.min_size {
            writer.discard();
            return Ok(None);
        }

        let global_end = if size == 0 {
            hit.global_offset
        } else {
            hit.global_offset + size - 1
        };

        let validated = !truncated
            && walked.corrupt_declared_chunks == 0
            && walked.missing_declared_chunks == 0;

        let file = CarvedFile {
            run_id: ctx.run_id.to_string(),
            file_type: self.file_type().to_string(),
            path: rel_path,
            extension: self.extension.clone(),
            global_start: hit.global_offset,
            global_end,
            size,
            md5: md5_hex,
            sha256: sha256_hex,
            validated,
            truncated,
            errors,
            pattern_id: Some(hit.pattern_id.clone()),
            is_duplicate: false,
            duplicate_of_offset: None,
        };

        let artefact = EvtxArtefact {
            run_id: ctx.run_id.to_string(),
            offset: hit.global_offset,
            size,
            first_chunk: parsed.first_chunk,
            last_chunk: parsed.last_chunk,
            record_count_estimate,
            // log_name is encoded only in the on-disk filename (e.g.
            // `Microsoft-Windows-PowerShell%4Operational.evtx`), not in the
            // EVTX file body. For raw-image carving this is always `None`.
            log_name: None,
        };

        Ok(Some(PendingCarve::new(file, writer).with_post_metadata(
            vec![PostCarveMetadata::WindowsArtefact(
                WindowsArtefactRecord::Evtx(artefact),
            )],
        )))
    }
}

// ------------------------------------------------------------------
// Internal parsing
// ------------------------------------------------------------------

#[derive(Debug)]
struct FileHeader {
    first_chunk: u64,
    last_chunk: u64,
    chunk_count: u32,
}

fn read_file_header(
    evidence: &dyn EvidenceSource,
    offset: u64,
) -> Result<Option<Vec<u8>>, CarveError> {
    let mut buf = vec![0u8; FILE_HEADER_SIZE as usize];
    if !fill_at(evidence, offset, &mut buf)? {
        return Ok(None);
    }
    if &buf[..FILE_MAGIC.len()] != FILE_MAGIC {
        return Ok(None);
    }
    Ok(Some(buf))
}

fn fill_at(evidence: &dyn EvidenceSource, offset: u64, buf: &mut [u8]) -> Result<bool, CarveError> {
    let mut filled = 0usize;
    while filled < buf.len() {
        let read_offset = offset
            .checked_add(filled as u64)
            .ok_or_else(|| CarveError::Invalid("evtx read offset overflows u64".into()))?;
        let n = evidence
            .read_at(read_offset, &mut buf[filled..])
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n == 0 {
            return Ok(false);
        }
        filled += n;
    }
    Ok(true)
}

fn parse_file_header(buf: &[u8]) -> Option<FileHeader> {
    if buf.len() < FILE_HEADER_SIZE as usize {
        return None;
    }
    if &buf[..FILE_MAGIC.len()] != FILE_MAGIC {
        return None;
    }

    // EVTX v3.x always declares `header_block_size == 0x1000` (4096) and the
    // file layout treats the header as a fixed 4 KiB block. Revisit this
    // exact-match check if a future major version ever introduces a variable-
    // sized header.
    let header_block_size = read_u16(buf, FH_HEADER_BLOCK_SIZE_OFFSET)?;
    if u64::from(header_block_size) != FILE_HEADER_SIZE {
        return None;
    }

    let major = read_u16(buf, FH_MAJOR_VERSION_OFFSET)?;
    if major != SUPPORTED_MAJOR_VERSION {
        return None;
    }
    // Minor version (Win7=1, Win11=2, future minors are additive) is
    // informational only — accept any value within `major == 3`.

    let chunk_count_u16 = read_u16(buf, FH_CHUNK_COUNT_OFFSET)?;
    let chunk_count = u32::from(chunk_count_u16);
    if chunk_count == 0 {
        return None;
    }
    if chunk_count > MAX_PLAUSIBLE_CHUNK_COUNT {
        tracing::debug!(
            chunk_count,
            cap = MAX_PLAUSIBLE_CHUNK_COUNT,
            "evtx header rejected: declared chunk_count exceeds plausibility cap (independent of configured max_size)"
        );
        return None;
    }

    let first_chunk = read_u64(buf, FH_FIRST_CHUNK_OFFSET)?;
    let last_chunk = read_u64(buf, FH_LAST_CHUNK_OFFSET)?;
    // `last_chunk` is the index of the chunk containing the newest record
    // and must fit within the declared `chunk_count`. `first_chunk` is
    // normally 0 but in *circular* (overwrite) channels it advances past
    // `last_chunk` once the log wraps — we accept that and only bound it
    // against `chunk_count`.
    if last_chunk >= u64::from(chunk_count) || first_chunk >= u64::from(chunk_count) {
        return None;
    }

    Some(FileHeader {
        first_chunk,
        last_chunk,
        chunk_count,
    })
}

/// Result of scanning chunks after the file header.
#[derive(Debug)]
struct WalkedChunks {
    /// Total verified contiguous chunks present within the declared
    /// `chunk_count` extent.
    verified_chunks: u32,
    /// Sum of `(last_record - first_record + 1)` across every chunk that
    /// could be read and had a valid `ElfChnk\0` magic.
    record_count_estimate: u64,
    /// Number of remaining declared chunks after the first bad chunk magic.
    /// These chunks are not counted toward `verified_chunks`.
    corrupt_declared_chunks: u32,
    /// Number of remaining declared chunks that could not be header-verified
    /// because the evidence ended first.
    missing_declared_chunks: u32,
}

/// Walk every declared chunk header. The carved extent only includes
/// contiguous chunks with verified `ElfChnk\0` magic; the first missing or
/// corrupt declared chunk stops the walk to preserve output isolation.
fn walk_chunks(
    evidence: &dyn EvidenceSource,
    base_offset: u64,
    declared_chunk_count: u32,
) -> Result<WalkedChunks, CarveError> {
    let mut total_records: u64 = 0;
    let mut verified_chunks: u32 = 0;
    let mut corrupt_declared_chunks: u32 = 0;
    let mut missing_declared_chunks: u32 = 0;
    let mut buf = [0u8; 0x18];
    let mut i: u32 = 0;
    while i < declared_chunk_count {
        let chunk_offset = base_offset
            .checked_add(FILE_HEADER_SIZE)
            .and_then(|o| o.checked_add(u64::from(i) * CHUNK_SIZE))
            .ok_or_else(|| CarveError::Invalid("evtx chunk offset overflows u64".into()))?;
        if !fill_at(evidence, chunk_offset, &mut buf)? {
            missing_declared_chunks = declared_chunk_count.saturating_sub(i);
            break;
        }
        if &buf[..CHUNK_MAGIC.len()] != CHUNK_MAGIC {
            corrupt_declared_chunks = declared_chunk_count.saturating_sub(i);
            break;
        }
        verified_chunks = i + 1;
        if let Some(first_record) = read_u64(&buf, CH_FIRST_RECORD_NUMBER_OFFSET)
            && let Some(last_record) = read_u64(&buf, CH_LAST_RECORD_NUMBER_OFFSET)
            && last_record >= first_record
        {
            let count = last_record
                .checked_sub(first_record)
                .and_then(|d| d.checked_add(1))
                .unwrap_or(0);
            total_records = total_records.saturating_add(count);
        }
        i += 1;
    }
    Ok(WalkedChunks {
        verified_chunks,
        record_count_estimate: total_records,
        corrupt_declared_chunks,
        missing_declared_chunks,
    })
}

fn read_u16(buf: &[u8], offset: usize) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let slice = buf.get(offset..end)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u64(buf: &[u8], offset: usize) -> Option<u64> {
    let end = offset.checked_add(8)?;
    let slice = buf.get(offset..end)?;
    Some(u64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_chunk(first_record: u64, last_record: u64) -> Vec<u8> {
        let mut chunk = vec![0u8; CHUNK_SIZE as usize];
        chunk[0..8].copy_from_slice(CHUNK_MAGIC);
        chunk[CH_FIRST_RECORD_NUMBER_OFFSET..CH_FIRST_RECORD_NUMBER_OFFSET + 8]
            .copy_from_slice(&first_record.to_le_bytes());
        chunk[CH_LAST_RECORD_NUMBER_OFFSET..CH_LAST_RECORD_NUMBER_OFFSET + 8]
            .copy_from_slice(&last_record.to_le_bytes());
        chunk
    }

    fn build_header(chunk_count: u16, first_chunk: u64, last_chunk: u64) -> Vec<u8> {
        let mut header = vec![0u8; FILE_HEADER_SIZE as usize];
        header[0..8].copy_from_slice(FILE_MAGIC);
        header[FH_FIRST_CHUNK_OFFSET..FH_FIRST_CHUNK_OFFSET + 8]
            .copy_from_slice(&first_chunk.to_le_bytes());
        header[FH_LAST_CHUNK_OFFSET..FH_LAST_CHUNK_OFFSET + 8]
            .copy_from_slice(&last_chunk.to_le_bytes());
        header[FH_MINOR_VERSION_OFFSET..FH_MINOR_VERSION_OFFSET + 2]
            .copy_from_slice(&1u16.to_le_bytes());
        header[FH_MAJOR_VERSION_OFFSET..FH_MAJOR_VERSION_OFFSET + 2]
            .copy_from_slice(&SUPPORTED_MAJOR_VERSION.to_le_bytes());
        header[FH_HEADER_BLOCK_SIZE_OFFSET..FH_HEADER_BLOCK_SIZE_OFFSET + 2]
            .copy_from_slice(&(FILE_HEADER_SIZE as u16).to_le_bytes());
        header[FH_CHUNK_COUNT_OFFSET..FH_CHUNK_COUNT_OFFSET + 2]
            .copy_from_slice(&chunk_count.to_le_bytes());
        header
    }

    #[test]
    fn parses_minimal_single_chunk_header() {
        let header = build_header(1, 0, 0);
        let parsed = parse_file_header(&header).expect("parse");
        assert_eq!(parsed.chunk_count, 1);
        assert_eq!(parsed.first_chunk, 0);
        assert_eq!(parsed.last_chunk, 0);
    }

    #[test]
    fn rejects_zero_chunk_count() {
        let header = build_header(0, 0, 0);
        assert!(parse_file_header(&header).is_none());
    }

    #[test]
    fn rejects_unsupported_major_version() {
        let mut header = build_header(1, 0, 0);
        header[FH_MAJOR_VERSION_OFFSET..FH_MAJOR_VERSION_OFFSET + 2]
            .copy_from_slice(&4u16.to_le_bytes());
        assert!(parse_file_header(&header).is_none());
    }

    #[test]
    fn accepts_minor_version_two() {
        let mut header = build_header(1, 0, 0);
        header[FH_MINOR_VERSION_OFFSET..FH_MINOR_VERSION_OFFSET + 2]
            .copy_from_slice(&2u16.to_le_bytes());
        assert!(parse_file_header(&header).is_some());
    }

    #[test]
    fn rejects_last_chunk_out_of_range() {
        let header = build_header(1, 0, 5);
        assert!(parse_file_header(&header).is_none());
    }

    #[test]
    fn rejects_first_chunk_out_of_range() {
        let header = build_header(2, 2, 1);
        assert!(parse_file_header(&header).is_none());
    }

    #[test]
    fn accepts_circular_first_after_last() {
        let header = build_header(4, 3, 1);
        let parsed = parse_file_header(&header).expect("parse circular header");
        assert_eq!(parsed.first_chunk, 3);
        assert_eq!(parsed.last_chunk, 1);
        assert_eq!(parsed.chunk_count, 4);
    }

    #[test]
    fn rejects_bad_header_block_size() {
        let mut header = build_header(1, 0, 0);
        header[FH_HEADER_BLOCK_SIZE_OFFSET..FH_HEADER_BLOCK_SIZE_OFFSET + 2]
            .copy_from_slice(&2048u16.to_le_bytes());
        assert!(parse_file_header(&header).is_none());
    }

    /// Tiny in-memory `EvidenceSource` for chunk-walk tests.
    struct Memory(Vec<u8>);
    impl EvidenceSource for Memory {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(
            &self,
            offset: u64,
            buf: &mut [u8],
        ) -> Result<usize, crate::evidence::EvidenceError> {
            if offset as usize >= self.0.len() {
                return Ok(0);
            }
            let start = offset as usize;
            let end = (start + buf.len()).min(self.0.len());
            let n = end - start;
            buf[..n].copy_from_slice(&self.0[start..end]);
            Ok(n)
        }
    }

    /// In-memory `EvidenceSource` that deliberately returns short reads away
    /// from EOF, matching layered evidence sources that cannot guarantee a
    /// full read in one call.
    struct ShortReadMemory {
        data: Vec<u8>,
        max_read: usize,
    }

    impl ShortReadMemory {
        fn new(data: Vec<u8>, max_read: usize) -> Self {
            Self {
                data,
                max_read: max_read.max(1),
            }
        }
    }

    impl EvidenceSource for ShortReadMemory {
        fn len(&self) -> u64 {
            self.data.len() as u64
        }

        fn read_at(
            &self,
            offset: u64,
            buf: &mut [u8],
        ) -> Result<usize, crate::evidence::EvidenceError> {
            if offset as usize >= self.data.len() {
                return Ok(0);
            }
            let start = offset as usize;
            let requested = buf.len().min(self.max_read);
            let end = (start + requested).min(self.data.len());
            let n = end - start;
            buf[..n].copy_from_slice(&self.data[start..end]);
            Ok(n)
        }
    }

    #[test]
    fn pre_validate_accepts_non_eof_short_reads() {
        let mut data = build_header(1, 0, 0);
        data.extend(build_chunk(1, 1));
        let mem = ShortReadMemory::new(data, 3);
        let handler = EvtxCarveHandler::new(
            "evtx".to_string(),
            FILE_HEADER_SIZE + CHUNK_SIZE,
            FILE_HEADER_SIZE + CHUNK_SIZE,
        );

        assert_eq!(
            handler.pre_validate(&mem, 0).expect("pre_validate"),
            PreValidation::Proceed
        );
    }

    #[test]
    fn walk_chunks_handles_non_eof_short_reads() {
        let mut data = build_header(2, 0, 1);
        data.extend(build_chunk(1, 3));
        data.extend(build_chunk(4, 5));
        let mem = ShortReadMemory::new(data, 5);

        let walked = walk_chunks(&mem, 0, 2).expect("walk");

        assert_eq!(walked.verified_chunks, 2);
        assert_eq!(walked.record_count_estimate, 5);
        assert_eq!(walked.corrupt_declared_chunks, 0);
        assert_eq!(walked.missing_declared_chunks, 0);
    }

    #[test]
    fn walks_chunks_and_sums_record_counts() {
        // Header + 3 chunks: counts 5, 0 (empty: last < first), 7 -> 12 total.
        let mut data = build_header(3, 0, 2);
        data.extend(build_chunk(1, 5)); // 5 records
        data.extend(build_chunk(6, 5)); // empty (synthetic sentinel)
        data.extend(build_chunk(6, 12)); // 7 records
        let mem = Memory(data);
        let walked = walk_chunks(&mem, 0, 3).expect("walk");
        assert_eq!(walked.record_count_estimate, 12);
        assert_eq!(walked.verified_chunks, 3);
    }

    #[test]
    fn walk_stops_at_evidence_boundary() {
        // Declare 3 chunks but only provide 2.
        let mut data = build_header(3, 0, 2);
        data.extend(build_chunk(1, 4)); // 4 records
        data.extend(build_chunk(5, 7)); // 3 records
        let mem = Memory(data);
        let walked = walk_chunks(&mem, 0, 3).expect("walk");
        assert_eq!(walked.record_count_estimate, 7);
        assert_eq!(walked.verified_chunks, 2);
        assert_eq!(walked.missing_declared_chunks, 1);
    }

    #[test]
    fn walk_stops_at_corrupt_chunk_magic() {
        let mut data = build_header(2, 0, 1);
        let mut bad = build_chunk(1, 5);
        bad[0] = b'X'; // break magic
        data.extend(bad);
        data.extend(build_chunk(6, 10)); // 5 records
        let mem = Memory(data);
        let walked = walk_chunks(&mem, 0, 2).expect("walk");
        assert_eq!(walked.record_count_estimate, 0);
        assert_eq!(walked.verified_chunks, 0);
        assert_eq!(walked.corrupt_declared_chunks, 2);
    }

    #[test]
    fn walk_stops_at_declared_count_when_extra_chunks_present() {
        // Declared chunk_count=2 but 4 physical ElfChnk\0 chunks on disk.
        let mut data = build_header(2, 0, 1);
        data.extend(build_chunk(1, 5)); // 5
        data.extend(build_chunk(6, 7)); // 2
        data.extend(build_chunk(8, 10)); // 3 (past declared)
        data.extend(build_chunk(11, 11)); // 1 (past declared)
        // Trailing zero block ends the on-disk file.
        data.extend(vec![0u8; CHUNK_SIZE as usize]);
        let mem = Memory(data);
        let walked = walk_chunks(&mem, 0, 2).expect("walk");
        assert_eq!(walked.verified_chunks, 2);
        assert_eq!(walked.record_count_estimate, 7);
        assert_eq!(walked.corrupt_declared_chunks, 0);
        assert_eq!(walked.missing_declared_chunks, 0);
    }
}
