//! OGG container carving handler.
//!
//! Ogg streams consist of pages with a fixed header, lacing table, and page data.
//! We walk pages validating CRC32, codec identification, and serial number
//! consistency until an end-of-stream flag is observed.

use crate::carve::{
    CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext, PreValidation,
    output_path,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

/// Maximum number of OGG pages to process.
const MAX_OGG_PAGES: u64 = 100_000;
/// Maximum data size per OGG page (255 segments × 255 bytes = 65,025).
const MAX_PAGE_DATA_SIZE: u64 = 65_025;
/// Minimum number of valid pages required before committing output.
const MIN_PAGE_COUNT: u64 = 2;

/// OGG CRC32 lookup table using polynomial 0x04C11DB7 (CRC-32/MPEG-2).
/// Initial value = 0, no final XOR.
const OGG_CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = (i as u32) << 24;
        let mut j = 0;
        while j < 8 {
            if crc & 0x8000_0000 != 0 {
                crc = (crc << 1) ^ 0x04C1_1DB7;
            } else {
                crc <<= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

/// Raw page read from evidence: (header, segment_table, page_data, total_page_size).
type RawPage = (Vec<u8>, Vec<u8>, Vec<u8>, u64);

/// Verify the CRC32 of a complete Ogg page.
/// `header` is the 27-byte page header (with the stored CRC at bytes 22..26).
/// Returns `true` if the computed CRC matches the stored CRC.
fn verify_page_crc(header: &[u8], segment_table: &[u8], page_data: &[u8]) -> bool {
    debug_assert_eq!(header.len(), 27, "OGG page header must be 27 bytes");
    // Build a copy of the header with the CRC field zeroed out
    let mut hdr = [0u8; 27];
    hdr.copy_from_slice(header);
    hdr[22] = 0;
    hdr[23] = 0;
    hdr[24] = 0;
    hdr[25] = 0;

    let stored_crc = u32::from_le_bytes([header[22], header[23], header[24], header[25]]);

    let mut crc: u32 = 0;
    for &b in &hdr {
        crc = (crc << 8) ^ OGG_CRC_TABLE[((crc >> 24) as u8 ^ b) as usize];
    }
    for &b in segment_table {
        crc = (crc << 8) ^ OGG_CRC_TABLE[((crc >> 24) as u8 ^ b) as usize];
    }
    for &b in page_data {
        crc = (crc << 8) ^ OGG_CRC_TABLE[((crc >> 24) as u8 ^ b) as usize];
    }

    crc == stored_crc
}

/// Read a full Ogg page from evidence at the given offset without writing.
/// Returns `(header, segment_table, page_data, total_page_size)` or `None` if truncated.
fn read_page_from_evidence(
    evidence: &dyn EvidenceSource,
    offset: u64,
) -> Result<Option<RawPage>, CarveError> {
    let mut header = vec![0u8; 27];
    let n = evidence
        .read_at(offset, &mut header)
        .map_err(|e| CarveError::Evidence(e.to_string()))?;
    if n < 27 {
        return Ok(None);
    }

    let segment_count = header[26] as usize;
    let mut seg_table = vec![0u8; segment_count];
    if segment_count > 0 {
        let n = evidence
            .read_at(offset + 27, &mut seg_table)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < segment_count {
            return Ok(None);
        }
    }

    let data_len: u64 = seg_table.iter().map(|&s| s as u64).sum();
    if data_len > MAX_PAGE_DATA_SIZE {
        return Ok(None);
    }

    let mut page_data = vec![0u8; data_len as usize];
    if data_len > 0 {
        let n = evidence
            .read_at(offset + 27 + segment_count as u64, &mut page_data)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < data_len as usize {
            return Ok(None);
        }
    }

    let total = 27 + segment_count as u64 + data_len;
    Ok(Some((header, seg_table, page_data, total)))
}

/// Check whether the BOS page data starts with a known codec signature.
fn is_known_codec(page_data: &[u8]) -> bool {
    if page_data.len() >= 7 && &page_data[0..7] == b"\x01vorbis" {
        return true;
    }
    if page_data.len() >= 8 && &page_data[0..8] == b"OpusHead" {
        return true;
    }
    if page_data.len() >= 5 && &page_data[0..5] == b"\x7fFLAC" {
        return true;
    }
    if page_data.len() >= 7 && &page_data[0..7] == b"\x80theora" {
        return true;
    }
    if page_data.len() >= 8 && &page_data[0..8] == b"Speex   " {
        return true;
    }
    false
}

pub struct OggCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl OggCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for OggCarveHandler {
    fn file_type(&self) -> &str {
        "ogg"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        // Read the full first page and verify its CRC32 to reject most false positives
        let page = read_page_from_evidence(evidence, offset)?;
        let (header, seg_table, page_data, _total) = match page {
            Some(p) => p,
            None => return Ok(PreValidation::Reject("truncated first page".to_string())),
        };

        if &header[0..4] != b"OggS" {
            return Ok(PreValidation::Reject("ogg signature mismatch".to_string()));
        }
        if header[4] != 0 {
            return Ok(PreValidation::Reject("ogg version unsupported".to_string()));
        }

        // First page must be BOS
        if header[5] & 0x02 == 0 {
            return Ok(PreValidation::Reject("first page not BOS".to_string()));
        }

        // Verify CRC32 of the first page
        if !verify_page_crc(&header, &seg_table, &page_data) {
            return Ok(PreValidation::Reject(
                "first page CRC32 mismatch".to_string(),
            ));
        }

        // Verify known codec signature in BOS page data
        if !is_known_codec(&page_data) {
            return Ok(PreValidation::Reject(
                "unknown codec in BOS page".to_string(),
            ));
        }

        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let mut stream = CarveStream::new(ctx, hit.global_offset, self.max_size, full_path.clone());

        let mut validated = false;
        let mut truncated = false;
        let mut errors = Vec::new();

        let result: Result<u64, CarveError> = (|| {
            let mut pages = 0u64;
            let mut expected_serial: Option<u32> = None;

            loop {
                let header = stream.read_exact(27)?;
                if &header[0..4] != b"OggS" {
                    return Err(CarveError::Invalid(
                        "ogg page signature mismatch".to_string(),
                    ));
                }
                if header[4] != 0 {
                    return Err(CarveError::Invalid("ogg version unsupported".to_string()));
                }

                let header_type = header[5];
                let serial = u32::from_le_bytes([header[14], header[15], header[16], header[17]]);
                let segment_count = header[26] as usize;

                let segment_table = stream.read_exact(segment_count)?;
                let data_len: u64 = segment_table.iter().map(|&s| s as u64).sum();

                if data_len > MAX_PAGE_DATA_SIZE {
                    return Err(CarveError::Invalid("ogg page data too large".to_string()));
                }

                let page_data = if data_len > 0 {
                    stream.read_exact(data_len as usize)?
                } else {
                    Vec::new()
                };

                // Verify CRC32
                let crc_ok = verify_page_crc(&header, &segment_table, &page_data);

                if pages == 0 {
                    // First page: CRC failure is fatal (reject entirely)
                    if !crc_ok {
                        return Err(CarveError::Invalid("first page CRC32 mismatch".to_string()));
                    }
                    // Must be BOS
                    if header_type & 0x02 == 0 {
                        return Err(CarveError::Invalid("first page not BOS".to_string()));
                    }
                    // Codec identification
                    if !is_known_codec(&page_data) {
                        return Err(CarveError::Invalid("unknown codec in BOS page".to_string()));
                    }
                    expected_serial = Some(serial);
                } else {
                    // Subsequent pages: CRC mismatch terminates stream
                    if !crc_ok {
                        // We have already written this page data, but it's bad.
                        // Terminate — the valid data ends before this page.
                        // We can't undo what CarveStream already wrote, so the
                        // output includes the invalid trailing page.
                        truncated = true;
                        errors.push("CRC32 mismatch on subsequent page (output includes invalid trailing page)".to_string());
                        break;
                    }
                    // Serial number consistency
                    if let Some(exp) = expected_serial
                        && serial != exp
                    {
                        truncated = true;
                        errors.push("serial number mismatch".to_string());
                        break;
                    }
                }

                pages += 1;
                if header_type & 0x04 != 0 {
                    validated = true;
                    break;
                }
                if pages > MAX_OGG_PAGES {
                    return Err(CarveError::Invalid("ogg page limit exceeded".to_string()));
                }
            }

            // Minimum page count check
            if pages < MIN_PAGE_COUNT {
                return Err(CarveError::Invalid("too few valid pages".to_string()));
            }

            Ok(stream.bytes_written())
        })();

        if let Err(err) = result {
            match err {
                CarveError::Truncated | CarveError::Eof => {
                    truncated = true;
                    errors.push(err.to_string());
                }
                CarveError::Invalid(_) => {
                    stream.discard();
                    return Ok(None);
                }
                other => return Err(other),
            }
        }

        let (size, md5_hex, sha256_hex) = stream.finish()?;

        if size < self.min_size {
            let _ = std::fs::remove_file(&full_path);
            return Ok(None);
        }

        if self.max_size > 0 && size >= self.max_size {
            truncated = true;
            if !errors.iter().any(|e| e.contains("max_size")) {
                errors.push("max_size reached".to_string());
            }
        }

        let global_end = if size == 0 {
            hit.global_offset
        } else {
            hit.global_offset + size - 1
        };

        Ok(Some(CarvedFile {
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
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::carve::{CarveHandler, ExtractionContext};
    use crate::evidence::{EvidenceError, EvidenceSource};
    use crate::scanner::NormalizedHit;
    use tempfile::tempdir;

    struct SliceEvidence {
        data: Vec<u8>,
    }

    impl EvidenceSource for SliceEvidence {
        fn len(&self) -> u64 {
            self.data.len() as u64
        }

        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, EvidenceError> {
            if offset as usize >= self.data.len() {
                return Ok(0);
            }
            let max = self.data.len() - offset as usize;
            let to_copy = buf.len().min(max);
            buf[..to_copy].copy_from_slice(&self.data[offset as usize..offset as usize + to_copy]);
            Ok(to_copy)
        }
    }

    /// Build an Ogg page with valid CRC32.
    /// `header_type`: flags byte (0x02 = BOS, 0x04 = EOS, etc.)
    /// `serial`: stream serial number
    /// `seq`: page sequence number
    /// `granule`: granule position
    /// `page_data`: payload
    fn build_ogg_page(
        header_type: u8,
        serial: u32,
        seq: u32,
        granule: u64,
        page_data: &[u8],
    ) -> Vec<u8> {
        // Build segment table: fill with 255-byte segments, final < 255 terminates packet
        let mut segments: Vec<u8> = Vec::new();
        let mut remaining = page_data.len();
        while remaining >= 255 {
            segments.push(255);
            remaining -= 255;
        }
        // Only add a terminating segment if there is a remainder (< 255),
        // or if the data is empty (0-byte segment). If remaining is 0 and we
        // already have segments, the packet continues into the next page per spec.
        if remaining > 0 || segments.is_empty() {
            segments.push(remaining as u8);
        }

        assert!(segments.len() <= 255, "too many segments for one OGG page");
        let segment_count = segments.len() as u8;

        // Build header with CRC zeroed
        let mut header = Vec::with_capacity(27);
        header.extend_from_slice(b"OggS"); // 0..4 capture pattern
        header.push(0); // 4: version
        header.push(header_type); // 5: header type
        header.extend_from_slice(&granule.to_le_bytes()); // 6..14: granule position
        header.extend_from_slice(&serial.to_le_bytes()); // 14..18: serial
        header.extend_from_slice(&seq.to_le_bytes()); // 18..22: page sequence
        header.extend_from_slice(&[0u8; 4]); // 22..26: CRC (zeroed for computation)
        header.push(segment_count); // 26: number of segments

        // Compute CRC over header + segment_table + page_data
        let mut crc: u32 = 0;
        for &b in &header {
            crc = (crc << 8) ^ OGG_CRC_TABLE[((crc >> 24) as u8 ^ b) as usize];
        }
        for &b in &segments {
            crc = (crc << 8) ^ OGG_CRC_TABLE[((crc >> 24) as u8 ^ b) as usize];
        }
        for &b in page_data {
            crc = (crc << 8) ^ OGG_CRC_TABLE[((crc >> 24) as u8 ^ b) as usize];
        }

        // Patch CRC into header
        let crc_bytes = crc.to_le_bytes();
        header[22] = crc_bytes[0];
        header[23] = crc_bytes[1];
        header[24] = crc_bytes[2];
        header[25] = crc_bytes[3];

        let mut page = header;
        page.extend_from_slice(&segments);
        page.extend_from_slice(page_data);
        page
    }

    /// Build a minimal valid two-page Vorbis OGG stream.
    fn build_vorbis_stream(serial: u32) -> Vec<u8> {
        // BOS page with Vorbis identification header
        let mut vorbis_id = Vec::new();
        vorbis_id.extend_from_slice(b"\x01vorbis");
        // Minimal Vorbis identification header fields (23 bytes after signature)
        vorbis_id.extend_from_slice(&0u32.to_le_bytes()); // vorbis version
        vorbis_id.push(2); // channels
        vorbis_id.extend_from_slice(&44100u32.to_le_bytes()); // sample rate
        vorbis_id.extend_from_slice(&0i32.to_le_bytes()); // bitrate max
        vorbis_id.extend_from_slice(&128000i32.to_le_bytes()); // bitrate nominal
        vorbis_id.extend_from_slice(&0i32.to_le_bytes()); // bitrate min
        vorbis_id.push(0x88); // blocksize 0/1
        vorbis_id.push(1); // framing flag

        let bos_page = build_ogg_page(0x02, serial, 0, 0, &vorbis_id);

        // EOS page with empty data
        let eos_page = build_ogg_page(0x04, serial, 1, 0, &[]);

        let mut stream = bos_page;
        stream.extend_from_slice(&eos_page);
        stream
    }

    fn make_hit() -> NormalizedHit {
        NormalizedHit {
            global_offset: 0,
            file_type_id: "ogg".to_string(),
            pattern_id: "ogg_sync".to_string(),
            chunk_data: None,
            chunk_start: 0,
        }
    }

    #[test]
    fn carves_valid_vorbis_stream() {
        let data = build_vorbis_stream(1);
        let evidence = SliceEvidence { data: data.clone() };
        let handler = OggCarveHandler::new("ogg".to_string(), 0, 0);
        let hit = make_hit();
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        let carved = carved.expect("should carve valid vorbis stream");
        assert!(carved.validated);
        assert_eq!(carved.size, data.len() as u64);
    }

    #[test]
    fn carves_opus_stream() {
        let mut opus_head = Vec::new();
        opus_head.extend_from_slice(b"OpusHead");
        opus_head.extend_from_slice(&[1, 2, 0x38, 0x01, 0x80, 0xBB, 0x00, 0x00, 0x00, 0x00, 0x00]);

        let serial = 42u32;
        let bos_page = build_ogg_page(0x02, serial, 0, 0, &opus_head);
        let eos_page = build_ogg_page(0x04, serial, 1, 0, &[]);
        let mut data = bos_page;
        data.extend_from_slice(&eos_page);

        let evidence = SliceEvidence { data: data.clone() };
        let handler = OggCarveHandler::new("ogg".to_string(), 0, 0);
        let hit = make_hit();
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        let carved = carved.expect("should carve opus stream");
        assert!(carved.validated);
    }

    #[test]
    fn rejects_unknown_codec() {
        let serial = 1u32;
        let bos_page = build_ogg_page(0x02, serial, 0, 0, b"UnknownCodec");
        let eos_page = build_ogg_page(0x04, serial, 1, 0, &[]);
        let mut data = bos_page;
        data.extend_from_slice(&eos_page);

        let evidence = SliceEvidence { data };
        let handler = OggCarveHandler::new("ogg".to_string(), 0, 0);
        let hit = make_hit();
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        assert!(carved.is_none(), "should reject unknown codec");
    }

    #[test]
    fn rejects_bad_first_page_crc() {
        let mut data = build_vorbis_stream(1);
        // Corrupt the CRC of the first page (bytes 22..26)
        data[22] ^= 0xFF;

        let evidence = SliceEvidence { data };
        let handler = OggCarveHandler::new("ogg".to_string(), 0, 0);
        let hit = make_hit();
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        assert!(carved.is_none(), "should reject bad first page CRC");
    }

    #[test]
    fn pre_validate_rejects_bad_crc() {
        let mut data = build_vorbis_stream(1);
        data[22] ^= 0xFF; // corrupt CRC

        let evidence = SliceEvidence { data };
        let handler = OggCarveHandler::new("ogg".to_string(), 0, 0);

        let result = handler.pre_validate(&evidence, 0).expect("pre_validate");
        match result {
            PreValidation::Reject(msg) => assert!(msg.contains("CRC32"), "msg: {msg}"),
            PreValidation::Proceed => panic!("should have rejected bad CRC"),
        }
    }

    #[test]
    fn pre_validate_rejects_unknown_codec() {
        let page = build_ogg_page(0x02, 1, 0, 0, b"BadCodecXX");
        let evidence = SliceEvidence { data: page };
        let handler = OggCarveHandler::new("ogg".to_string(), 0, 0);

        let result = handler.pre_validate(&evidence, 0).expect("pre_validate");
        match result {
            PreValidation::Reject(msg) => assert!(msg.contains("codec"), "msg: {msg}"),
            PreValidation::Proceed => panic!("should have rejected unknown codec"),
        }
    }

    #[test]
    fn rejects_serial_number_mismatch() {
        let serial_a = 100u32;
        let serial_b = 200u32;

        let mut vorbis_id = Vec::new();
        vorbis_id.extend_from_slice(b"\x01vorbis");
        vorbis_id.extend_from_slice(&0u32.to_le_bytes());
        vorbis_id.push(2);
        vorbis_id.extend_from_slice(&44100u32.to_le_bytes());
        vorbis_id.extend_from_slice(&0i32.to_le_bytes());
        vorbis_id.extend_from_slice(&128000i32.to_le_bytes());
        vorbis_id.extend_from_slice(&0i32.to_le_bytes());
        vorbis_id.push(0x88);
        vorbis_id.push(1);

        let bos_page = build_ogg_page(0x02, serial_a, 0, 0, &vorbis_id);
        // Second page with comment data and DIFFERENT serial
        let comment_page = build_ogg_page(0x00, serial_b, 1, 0, b"\x03vorbis");
        let eos_page = build_ogg_page(0x04, serial_a, 2, 0, &[]);

        let mut data = bos_page;
        data.extend_from_slice(&comment_page);
        data.extend_from_slice(&eos_page);

        let evidence = SliceEvidence { data };
        let handler = OggCarveHandler::new("ogg".to_string(), 0, 0);
        let hit = make_hit();
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        // Page 0 (serial_a): passes, pages = 1
        // Page 1 (serial_b): serial mismatch → break
        // pages(1) < MIN_PAGE_COUNT(2) → Invalid → file deleted → None
        assert!(
            carved.is_none(),
            "should reject: serial mismatch with too few valid pages"
        );
    }

    #[test]
    fn rejects_too_few_pages() {
        // Build a single BOS+EOS page (combined flags) — only 1 page total
        let mut vorbis_id = Vec::new();
        vorbis_id.extend_from_slice(b"\x01vorbis");
        vorbis_id.extend_from_slice(&0u32.to_le_bytes());
        vorbis_id.push(2);
        vorbis_id.extend_from_slice(&44100u32.to_le_bytes());
        vorbis_id.extend_from_slice(&0i32.to_le_bytes());
        vorbis_id.extend_from_slice(&128000i32.to_le_bytes());
        vorbis_id.extend_from_slice(&0i32.to_le_bytes());
        vorbis_id.push(0x88);
        vorbis_id.push(1);

        // BOS + EOS in one page
        let page = build_ogg_page(0x02 | 0x04, 1, 0, 0, &vorbis_id);

        let evidence = SliceEvidence { data: page };
        let handler = OggCarveHandler::new("ogg".to_string(), 0, 0);
        let hit = make_hit();
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        assert!(carved.is_none(), "should reject single-page stream");
    }

    #[test]
    fn rejects_excessive_page_data() {
        // Build a valid BOS page, then a page with oversized data field
        let mut vorbis_id = Vec::new();
        vorbis_id.extend_from_slice(b"\x01vorbis");
        vorbis_id.extend_from_slice(&0u32.to_le_bytes());
        vorbis_id.push(2);
        vorbis_id.extend_from_slice(&44100u32.to_le_bytes());
        vorbis_id.extend_from_slice(&0i32.to_le_bytes());
        vorbis_id.extend_from_slice(&128000i32.to_le_bytes());
        vorbis_id.extend_from_slice(&0i32.to_le_bytes());
        vorbis_id.push(0x88);
        vorbis_id.push(1);

        let bos_page = build_ogg_page(0x02, 1, 0, 0, &vorbis_id);

        // Build a page at the max data size limit (255 × 255 = 65025) — should pass
        let big_data = vec![0xABu8; 65025];
        let big_page = build_ogg_page(0x00, 1, 1, 0, &big_data);
        let eos_page = build_ogg_page(0x04, 1, 2, 0, &[]);

        let mut data = bos_page;
        data.extend_from_slice(&big_page);
        data.extend_from_slice(&eos_page);

        let evidence = SliceEvidence { data };
        let handler = OggCarveHandler::new("ogg".to_string(), 0, 0);
        let hit = make_hit();
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        assert!(carved.is_some(), "should accept page at exact data limit");
    }

    #[test]
    fn rejects_mismatched_page_signature() {
        let mut vorbis_id = Vec::new();
        vorbis_id.extend_from_slice(b"\x01vorbis");
        vorbis_id.extend_from_slice(&0u32.to_le_bytes());
        vorbis_id.push(2);
        vorbis_id.extend_from_slice(&44100u32.to_le_bytes());
        vorbis_id.extend_from_slice(&0i32.to_le_bytes());
        vorbis_id.extend_from_slice(&128000i32.to_le_bytes());
        vorbis_id.extend_from_slice(&0i32.to_le_bytes());
        vorbis_id.push(0x88);
        vorbis_id.push(1);

        let bos_page = build_ogg_page(0x02, 1, 0, 0, &vorbis_id);

        // Invalid second page
        let mut garbage = Vec::new();
        garbage.extend_from_slice(b"XXXX");
        garbage.extend_from_slice(&[0u8; 23]);

        let mut data = bos_page;
        data.extend_from_slice(&garbage);

        let evidence = SliceEvidence { data };
        let handler = OggCarveHandler::new("ogg".to_string(), 0, 0);
        let hit = make_hit();
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        assert!(carved.is_none(), "should reject mismatched page signature");
    }

    #[test]
    fn crc32_known_vector() {
        // Verify the CRC implementation against a known page
        let page = build_ogg_page(0x02, 1, 0, 0, b"\x01vorbis");
        // The page we just built should have a valid CRC
        let header = &page[0..27];
        let seg_count = header[26] as usize;
        let seg_table = &page[27..27 + seg_count];
        let page_data = &page[27 + seg_count..];
        assert!(verify_page_crc(header, seg_table, page_data));

        // Corrupt one byte and verify CRC fails
        let mut corrupted = page.clone();
        let last = corrupted.len() - 1;
        corrupted[last] ^= 0x01;
        let header = &corrupted[0..27];
        let seg_table = &corrupted[27..27 + seg_count];
        let page_data = &corrupted[27 + seg_count..];
        assert!(!verify_page_crc(header, seg_table, page_data));
    }

    #[test]
    fn flac_codec_accepted() {
        let mut flac_header = Vec::new();
        flac_header.extend_from_slice(b"\x7fFLAC");
        flac_header.extend_from_slice(&[1, 0]); // version
        flac_header.extend_from_slice(&[0; 10]); // padding

        let serial = 7u32;
        let bos_page = build_ogg_page(0x02, serial, 0, 0, &flac_header);
        let eos_page = build_ogg_page(0x04, serial, 1, 0, &[]);
        let mut data = bos_page;
        data.extend_from_slice(&eos_page);

        let evidence = SliceEvidence { data: data.clone() };
        let handler = OggCarveHandler::new("ogg".to_string(), 0, 0);
        let hit = make_hit();
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        assert!(carved.is_some(), "should accept FLAC in OGG");
    }

    #[test]
    fn theora_codec_accepted() {
        let mut theora_header = Vec::new();
        theora_header.extend_from_slice(b"\x80theora");
        theora_header.extend_from_slice(&[3, 2]); // version
        theora_header.extend_from_slice(&[0; 30]); // padding

        let serial = 8u32;
        let bos_page = build_ogg_page(0x02, serial, 0, 0, &theora_header);
        let eos_page = build_ogg_page(0x04, serial, 1, 0, &[]);
        let mut data = bos_page;
        data.extend_from_slice(&eos_page);

        let evidence = SliceEvidence { data: data.clone() };
        let handler = OggCarveHandler::new("ogg".to_string(), 0, 0);
        let hit = make_hit();
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        assert!(carved.is_some(), "should accept Theora in OGG");
    }

    #[test]
    fn speex_codec_accepted() {
        let mut speex_header = Vec::new();
        speex_header.extend_from_slice(b"Speex   ");
        speex_header.extend_from_slice(&[1, 0, 0, 0]); // version
        speex_header.extend_from_slice(&[0; 20]); // padding

        let serial = 9u32;
        let bos_page = build_ogg_page(0x02, serial, 0, 0, &speex_header);
        let eos_page = build_ogg_page(0x04, serial, 1, 0, &[]);
        let mut data = bos_page;
        data.extend_from_slice(&eos_page);

        let evidence = SliceEvidence { data: data.clone() };
        let handler = OggCarveHandler::new("ogg".to_string(), 0, 0);
        let hit = make_hit();
        let dir = tempdir().expect("tempdir");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        assert!(carved.is_some(), "should accept Speex in OGG");
    }
}
