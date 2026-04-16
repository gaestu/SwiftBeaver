use sha2::{Digest, Sha256};

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, PreValidation, output_path,
    write_range,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const WAL_HEADER_LEN: u64 = 32;
const WAL_FRAME_HEADER_LEN: u64 = 24;
const WAL_MAGIC_1: u32 = 0x377F_0682;
const WAL_MAGIC_2: u32 = 0x377F_0683;
const WAL_VERSION: u32 = 3_007_000;

#[derive(Debug, Clone, Copy)]
enum ChecksumByteOrder {
    BigEndian,
    LittleEndian,
}

#[derive(Debug, Clone, Copy)]
struct WalHeader {
    page_size: u32,
    salt_1: u32,
    salt_2: u32,
    checksum_byte_order: ChecksumByteOrder,
    frame_checksum: [u32; 2],
}

#[derive(Debug)]
struct WalWalkResult {
    size: u64,
    frames: u32,
    checksum_mismatches: u32,
    truncated: bool,
    errors: Vec<String>,
}

pub struct SqliteWalCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
    max_consecutive_checksum_failures: u32,
}

impl SqliteWalCarveHandler {
    pub fn new(
        extension: String,
        min_size: u64,
        max_size: u64,
        max_consecutive_checksum_failures: u32,
    ) -> Self {
        Self {
            extension,
            min_size,
            max_size,
            max_consecutive_checksum_failures,
        }
    }
}

impl CarveHandler for SqliteWalCarveHandler {
    fn file_type(&self) -> &str {
        "sqlite_wal"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let mut buf = [0u8; 4];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < buf.len() {
            return Ok(PreValidation::Reject("truncated header".to_string()));
        }
        let magic = u32::from_be_bytes(buf);
        if magic != WAL_MAGIC_1 && magic != WAL_MAGIC_2 {
            return Ok(PreValidation::Reject(
                "sqlite wal magic mismatch".to_string(),
            ));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        let header_bytes = match read_exact_at(ctx, hit.global_offset, WAL_HEADER_LEN as usize)? {
            Some(bytes) => bytes,
            None => return Ok(None),
        };
        let header = match parse_wal_header(&header_bytes) {
            Some(header) => header,
            None => return Ok(None),
        };

        let walked = walk_frames(
            ctx,
            hit.global_offset,
            header,
            self.max_size,
            self.max_consecutive_checksum_failures,
        )?;
        if walked.frames == 0 {
            return Ok(None);
        }

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let mut md5 = md5::Context::new();
        let mut sha256 = Sha256::new();

        let mut truncated = walked.truncated;
        let mut errors = walked.errors;
        if walked.checksum_mismatches > 0 {
            errors.push(format!(
                "encountered {} WAL checksum mismatch frame(s); threshold controls stopping, not frame filtering",
                walked.checksum_mismatches
            ));
        }
        let end = hit.global_offset.saturating_add(walked.size);
        let (written, eof_truncated) = write_range(
            ctx,
            hit.global_offset,
            end,
            &full_path,
            &mut md5,
            &mut sha256,
        )?;
        if eof_truncated {
            truncated = true;
            if !errors.iter().any(|e| e.contains("eof")) {
                errors.push("eof before WAL end".to_string());
            }
        }

        if written < self.min_size {
            let _ = std::fs::remove_file(&full_path);
            return Ok(None);
        }

        let global_end = if written == 0 {
            hit.global_offset
        } else {
            hit.global_offset + written - 1
        };
        let validated = walked.frames > 0 && !truncated && walked.checksum_mismatches == 0;
        let md5_hex = format!("{:x}", md5.compute());
        let sha256_hex = hex::encode(sha256.finalize());

        Ok(Some(CarvedFile {
            run_id: ctx.run_id.to_string(),
            file_type: self.file_type().to_string(),
            path: rel_path,
            extension: self.extension.clone(),
            global_start: hit.global_offset,
            global_end,
            size: written,
            md5: Some(md5_hex),
            sha256: Some(sha256_hex),
            validated,
            truncated,
            errors,
            pattern_id: Some(hit.pattern_id.clone()),
        }))
    }
}

fn read_exact_at(
    ctx: &ExtractionContext,
    offset: u64,
    len: usize,
) -> Result<Option<Vec<u8>>, CarveError> {
    let mut buf = vec![0u8; len];
    let n = ctx
        .evidence
        .read_at(offset, &mut buf)
        .map_err(|e| CarveError::Evidence(e.to_string()))?;
    if n < len {
        return Ok(None);
    }
    Ok(Some(buf))
}

fn parse_wal_header(bytes: &[u8]) -> Option<WalHeader> {
    if bytes.len() < WAL_HEADER_LEN as usize {
        return None;
    }
    let magic = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if (magic & 0xFFFF_FFFE) != WAL_MAGIC_1 {
        return None;
    }
    let checksum_byte_order = if magic == WAL_MAGIC_2 {
        ChecksumByteOrder::BigEndian
    } else if magic == WAL_MAGIC_1 {
        ChecksumByteOrder::LittleEndian
    } else {
        return None;
    };

    let version = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if version != WAL_VERSION {
        return None;
    }

    // SQLite WAL uses the database page size here.
    let raw_page_size = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let page_size = if raw_page_size == 1 {
        65536
    } else {
        raw_page_size
    };
    if !(512..=65536).contains(&page_size) || !page_size.is_power_of_two() {
        return None;
    }

    let computed_header_checksum =
        wal_checksum_bytes(checksum_byte_order, &bytes[..24], [0u32, 0u32])?;
    let header_checksum = [
        u32::from_be_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
        u32::from_be_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]),
    ];
    if computed_header_checksum != header_checksum {
        return None;
    }

    Some(WalHeader {
        page_size,
        salt_1: u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
        salt_2: u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
        checksum_byte_order,
        frame_checksum: header_checksum,
    })
}

fn walk_frames(
    ctx: &ExtractionContext,
    start: u64,
    header: WalHeader,
    max_size: u64,
    max_consecutive_checksum_failures: u32,
) -> Result<WalWalkResult, CarveError> {
    let evidence_len = ctx.evidence.len();
    let hard_end = if max_size > 0 {
        start.saturating_add(max_size).min(evidence_len)
    } else {
        evidence_len
    };

    let mut offset = start.saturating_add(WAL_HEADER_LEN);
    let mut frames = 0u32;
    let mut checksum_mismatches = 0u32;
    let mut truncated = false;
    let mut errors = Vec::new();
    let mut consecutive_checksum_failures = 0u32;
    let mut rolling_checksum = header.frame_checksum;

    let frame_size = WAL_FRAME_HEADER_LEN.saturating_add(header.page_size as u64);
    while offset.saturating_add(WAL_FRAME_HEADER_LEN) <= hard_end {
        let frame_header = match read_exact_at(ctx, offset, WAL_FRAME_HEADER_LEN as usize)? {
            Some(bytes) => bytes,
            None => break,
        };

        let page_no = u32::from_be_bytes([
            frame_header[0],
            frame_header[1],
            frame_header[2],
            frame_header[3],
        ]);
        let frame_salt_1 = u32::from_be_bytes([
            frame_header[8],
            frame_header[9],
            frame_header[10],
            frame_header[11],
        ]);
        let frame_salt_2 = u32::from_be_bytes([
            frame_header[12],
            frame_header[13],
            frame_header[14],
            frame_header[15],
        ]);
        if page_no == 0 || frame_salt_1 != header.salt_1 || frame_salt_2 != header.salt_2 {
            break;
        }
        let frame_end = offset.saturating_add(frame_size);
        if frame_end > hard_end {
            truncated = true;
            if max_size > 0 && start.saturating_add(max_size) <= evidence_len {
                errors.push("max_size reached before WAL frame end".to_string());
            } else {
                errors.push("eof before WAL frame end".to_string());
            }
            break;
        }

        let page_data = match read_exact_at(
            ctx,
            offset.saturating_add(WAL_FRAME_HEADER_LEN),
            header.page_size as usize,
        )? {
            Some(data) => data,
            None => {
                truncated = true;
                errors.push("eof before WAL frame payload".to_string());
                break;
            }
        };

        let mut frame_checksum = match wal_checksum_bytes(
            header.checksum_byte_order,
            &frame_header[..8],
            rolling_checksum,
        ) {
            Some(ck) => ck,
            None => break,
        };
        frame_checksum =
            match wal_checksum_bytes(header.checksum_byte_order, &page_data, frame_checksum) {
                Some(ck) => ck,
                None => break,
            };
        let expected_checksum_1 = u32::from_be_bytes([
            frame_header[16],
            frame_header[17],
            frame_header[18],
            frame_header[19],
        ]);
        let expected_checksum_2 = u32::from_be_bytes([
            frame_header[20],
            frame_header[21],
            frame_header[22],
            frame_header[23],
        ]);
        if frame_checksum[0] != expected_checksum_1 || frame_checksum[1] != expected_checksum_2 {
            checksum_mismatches = checksum_mismatches.saturating_add(1);
            consecutive_checksum_failures = consecutive_checksum_failures.saturating_add(1);
            if consecutive_checksum_failures > max_consecutive_checksum_failures {
                errors.push(format!(
                    "checksum mismatch for {} consecutive WAL frame(s)",
                    consecutive_checksum_failures
                ));
                break;
            }
            offset = frame_end;
            continue;
        }
        consecutive_checksum_failures = 0;
        rolling_checksum = frame_checksum;

        frames = frames.saturating_add(1);
        offset = frame_end;
    }

    Ok(WalWalkResult {
        size: offset.saturating_sub(start),
        frames,
        checksum_mismatches,
        truncated,
        errors,
    })
}

fn read_u32(order: ChecksumByteOrder, bytes: &[u8]) -> Option<u32> {
    if bytes.len() != 4 {
        return None;
    }
    Some(match order {
        ChecksumByteOrder::BigEndian => {
            u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        }
        ChecksumByteOrder::LittleEndian => {
            u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        }
    })
}

fn wal_checksum_bytes(
    order: ChecksumByteOrder,
    data: &[u8],
    mut checksum: [u32; 2],
) -> Option<[u32; 2]> {
    if data.len() < 8 || !data.len().is_multiple_of(8) {
        return None;
    }

    for pair in data.chunks_exact(8) {
        let x0 = read_u32(order, &pair[0..4])?;
        let x1 = read_u32(order, &pair[4..8])?;
        checksum[0] = checksum[0].wrapping_add(x0).wrapping_add(checksum[1]);
        checksum[1] = checksum[1].wrapping_add(x1).wrapping_add(checksum[0]);
    }
    Some(checksum)
}

#[cfg(test)]
mod tests {
    use super::{ChecksumByteOrder, SqliteWalCarveHandler, parse_wal_header, wal_checksum_bytes};
    use crate::carve::{CarveHandler, ExtractionContext};
    use crate::evidence::RawFileSource;
    use crate::scanner::NormalizedHit;

    fn build_header(magic: u32, page_size: u32) -> [u8; 32] {
        let mut header = [0u8; 32];
        header[0..4].copy_from_slice(&magic.to_be_bytes());
        header[4..8].copy_from_slice(&3007000u32.to_be_bytes());
        header[8..12].copy_from_slice(&page_size.to_be_bytes());
        header[16..20].copy_from_slice(&0xAABB_CCDDu32.to_be_bytes());
        header[20..24].copy_from_slice(&0x1122_3344u32.to_be_bytes());
        let order = if magic == 0x377F_0683 {
            ChecksumByteOrder::BigEndian
        } else {
            ChecksumByteOrder::LittleEndian
        };
        let cksum = wal_checksum_bytes(order, &header[..24], [0, 0]).expect("header checksum");
        header[24..28].copy_from_slice(&cksum[0].to_be_bytes());
        header[28..32].copy_from_slice(&cksum[1].to_be_bytes());
        header
    }

    fn build_wal_with_frames(frame_count: u32, truncate_last_frame_bytes: usize) -> Vec<u8> {
        let page_size = 4096u32;
        let salt_1: u32 = 0xAABB_CCDD;
        let salt_2: u32 = 0x1122_3344;
        let mut wal = vec![0u8; 32];
        wal[0..4].copy_from_slice(&0x377F_0682u32.to_be_bytes());
        wal[4..8].copy_from_slice(&3007000u32.to_be_bytes());
        wal[8..12].copy_from_slice(&page_size.to_be_bytes());
        wal[16..20].copy_from_slice(&salt_1.to_be_bytes());
        wal[20..24].copy_from_slice(&salt_2.to_be_bytes());
        let mut rolling = wal_checksum_bytes(ChecksumByteOrder::LittleEndian, &wal[..24], [0, 0])
            .expect("header checksum");
        wal[24..28].copy_from_slice(&rolling[0].to_be_bytes());
        wal[28..32].copy_from_slice(&rolling[1].to_be_bytes());

        for i in 0..frame_count {
            let mut frame = vec![0u8; 24 + page_size as usize];
            frame[0..4].copy_from_slice(&(i + 1).to_be_bytes());
            frame[8..12].copy_from_slice(&salt_1.to_be_bytes());
            frame[12..16].copy_from_slice(&salt_2.to_be_bytes());
            for b in frame[24..].iter_mut() {
                *b = (i + 1) as u8;
            }
            rolling = wal_checksum_bytes(ChecksumByteOrder::LittleEndian, &frame[0..8], rolling)
                .expect("frame header checksum");
            rolling = wal_checksum_bytes(ChecksumByteOrder::LittleEndian, &frame[24..], rolling)
                .expect("frame data checksum");
            frame[16..20].copy_from_slice(&rolling[0].to_be_bytes());
            frame[20..24].copy_from_slice(&rolling[1].to_be_bytes());
            wal.extend_from_slice(&frame);
        }

        if truncate_last_frame_bytes > 0 && wal.len() > truncate_last_frame_bytes {
            wal.truncate(wal.len() - truncate_last_frame_bytes);
        }

        wal
    }

    #[test]
    fn parses_valid_magic_values() {
        let mut header = build_header(0x377F_0682, 4096);
        assert!(parse_wal_header(&header).is_some());

        header = build_header(0x377F_0683, 4096);
        assert!(parse_wal_header(&header).is_some());
    }

    #[test]
    fn rejects_invalid_magic_value() {
        let header = build_header(0x1234_5678, 4096);
        assert!(parse_wal_header(&header).is_none());
    }

    #[test]
    fn stops_cleanly_on_truncated_frame() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output dir");

        let wal = build_wal_with_frames(1, 200);
        let input_path = temp_dir.path().join("wal.bin");
        std::fs::write(&input_path, &wal).expect("write wal");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
            deferred_buffer_bytes: 0,
        };
        let handler = SqliteWalCarveHandler::new("sqlite-wal".to_string(), 32, 0, 2);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "sqlite_wal".to_string(),
            pattern_id: "sqlite_wal_magic_82".to_string(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        assert!(carved.is_none(), "truncated first frame should be rejected");
    }

    #[test]
    fn rejects_repeated_checksum_failures() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output dir");

        let mut wal = build_wal_with_frames(3, 0);
        let mut cursor = 32usize;
        for _ in 0..3 {
            wal[cursor + 16..cursor + 20].copy_from_slice(&0u32.to_be_bytes());
            wal[cursor + 20..cursor + 24].copy_from_slice(&0u32.to_be_bytes());
            cursor += 24 + 4096;
        }

        let input_path = temp_dir.path().join("wal_bad_checksum.bin");
        std::fs::write(&input_path, &wal).expect("write wal");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
            deferred_buffer_bytes: 0,
        };
        let handler = SqliteWalCarveHandler::new("sqlite-wal".to_string(), 32, 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "sqlite_wal".to_string(),
            pattern_id: "sqlite_wal_magic_82".to_string(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        assert!(
            carved.is_none(),
            "expected repeated checksum failures to reject"
        );
    }
}
