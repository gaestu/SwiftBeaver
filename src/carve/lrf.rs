//! LRF (Sony BroadBand eBook) carving handler.
//!
//! Validates the 32-byte LRF header structure to reject false positives
//! from the short 4-byte magic `LRF\0`. Fields checked: version, root
//! object ID, number of objects, object index offset, and declared size.

use tracing::debug;

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, PendingCarve, PreValidation,
    create_hashers, finalize_hashers, output_path, write_range,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const LRF_MAGIC: [u8; 4] = [0x4C, 0x52, 0x46, 0x00];

/// Minimum header bytes needed for structural validation.
const LRF_HEADER_LEN: usize = 32;

/// Maximum plausible LRF version number (format was short-lived).
const MAX_VERSION: u16 = 10_000;

/// Maximum plausible number of objects in an LRF file.
const MAX_NUM_OBJECTS: u32 = 100_000;

pub struct LrfCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl LrfCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for LrfCarveHandler {
    fn file_type(&self) -> &str {
        "lrf"
    }

    fn is_fast(&self) -> bool {
        true
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
        if buf != LRF_MAGIC {
            return Ok(PreValidation::Reject("lrf magic mismatch".to_string()));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
        let header = read_exact_at(ctx, hit.global_offset, LRF_HEADER_LEN)
            .ok_or_else(|| CarveError::Invalid("lrf header too short".to_string()))?;
        if header[0..4] != LRF_MAGIC {
            return Ok(None);
        }

        // --- Structural validation ---

        // Version (bytes 4-5, LE u16): must be non-zero and plausible.
        let version = u16::from_le_bytes([header[4], header[5]]);
        if version == 0 || version > MAX_VERSION {
            debug!(
                offset = hit.global_offset,
                version, "lrf: rejected — version out of range"
            );
            return Ok(None);
        }

        // Declared file size (bytes 8-11, LE u32).
        let declared = u32::from_le_bytes([header[8], header[9], header[10], header[11]]) as u64;

        // Root object ID (bytes 16-19, LE u32): must be non-zero.
        let root_object_id = u32::from_le_bytes([header[16], header[17], header[18], header[19]]);
        if root_object_id == 0 {
            debug!(
                offset = hit.global_offset,
                "lrf: rejected — root object ID is zero"
            );
            return Ok(None);
        }

        // Number of objects (bytes 20-23, LE u32): must be > 0 and sane.
        let num_objects = u32::from_le_bytes([header[20], header[21], header[22], header[23]]);
        if num_objects == 0 || num_objects > MAX_NUM_OBJECTS {
            debug!(
                offset = hit.global_offset,
                num_objects, "lrf: rejected — num_objects out of range"
            );
            return Ok(None);
        }

        // Object index offset (bytes 24-31, LE u64): must be > 0.
        let obj_index_offset = u64::from_le_bytes([
            header[24], header[25], header[26], header[27], header[28], header[29], header[30],
            header[31],
        ]);
        if obj_index_offset == 0 {
            debug!(
                offset = hit.global_offset,
                "lrf: rejected — object index offset is zero"
            );
            return Ok(None);
        }

        // If declared size is known, the object index must fall within it.
        if declared > 0 && obj_index_offset >= declared {
            debug!(
                offset = hit.global_offset,
                declared, obj_index_offset, "lrf: rejected — object index offset >= declared size"
            );
            return Ok(None);
        }

        // --- Size determination ---
        // Reject when declared size is missing or exceeds max_size — do NOT
        // fall back to max_size, because a garbage size field is a strong
        // indicator of a false positive.
        if declared == 0 {
            debug!(
                offset = hit.global_offset,
                "lrf: rejected — declared size is zero"
            );
            return Ok(None);
        }
        if self.max_size > 0 && declared > self.max_size {
            debug!(
                offset = hit.global_offset,
                declared,
                max_size = self.max_size,
                "lrf: rejected — declared size exceeds max_size"
            );
            return Ok(None);
        }

        let total_end = hit.global_offset.saturating_add(declared);

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let (mut md5, mut sha256) = create_hashers(&ctx.hash_config);

        let (written, eof_truncated, mut writer) = write_range(
            ctx,
            hit.global_offset,
            total_end,
            &full_path,
            md5.as_mut(),
            sha256.as_mut(),
        )?;

        if written < self.min_size {
            writer.discard();
            return Ok(None);
        }

        let (md5_hex, sha256_hex) = finalize_hashers(md5, sha256);
        let global_end = if written == 0 {
            hit.global_offset
        } else {
            hit.global_offset + written - 1
        };

        // Only mark as validated when all structural checks passed and the
        // full declared size was written without truncation.
        let validated = !eof_truncated;

        Ok(Some(PendingCarve::new(
            CarvedFile {
                run_id: ctx.run_id.to_string(),
                file_type: self.file_type().to_string(),
                path: rel_path,
                extension: self.extension.clone(),
                global_start: hit.global_offset,
                global_end,
                size: written,
                md5: md5_hex,
                sha256: sha256_hex,
                validated,
                truncated: eof_truncated,
                errors: Vec::new(),
                pattern_id: Some(hit.pattern_id.clone()),
                is_duplicate: false,
                duplicate_of_offset: None,
            },
            writer,
        )))
    }
}

fn read_exact_at(ctx: &ExtractionContext, offset: u64, len: usize) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let n = ctx.evidence.read_at(offset, &mut buf).ok()?;
    if n < len {
        return None;
    }
    Some(buf)
}

/// Build a minimal valid 64-byte LRF buffer for testing.
#[cfg(test)]
fn make_valid_lrf(size: u32) -> Vec<u8> {
    let mut data = vec![0u8; size.max(64) as usize];
    // Magic
    data[0..4].copy_from_slice(&[0x4C, 0x52, 0x46, 0x00]);
    // Version = 1
    data[4..6].copy_from_slice(&1u16.to_le_bytes());
    // Declared file size
    data[8..12].copy_from_slice(&size.to_le_bytes());
    // Root object ID = 1
    data[16..20].copy_from_slice(&1u32.to_le_bytes());
    // Number of objects = 1
    data[20..24].copy_from_slice(&1u32.to_le_bytes());
    // Object index offset = 32 (right after header)
    data[24..32].copy_from_slice(&32u64.to_le_bytes());
    data
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

    fn hit_at(offset: u64) -> NormalizedHit {
        NormalizedHit {
            global_offset: offset,
            file_type_id: "lrf".to_string(),
            pattern_id: "lrf_header".to_string(),
            chunk_data: None,
            chunk_start: 0,
        }
    }

    #[test]
    fn carves_valid_lrf() {
        let data = make_valid_lrf(64);
        let evidence = SliceEvidence { data: data.clone() };
        let handler = LrfCarveHandler::new("lrf".to_string(), 0, 1_048_576);
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

        let carved = handler
            .process_hit(&hit_at(0), &ctx)
            .expect("process")
            .expect("carved")
            .flush()
            .expect("flush");
        assert_eq!(carved.size, 64);
        assert!(carved.validated);
        assert!(!carved.truncated);
    }

    #[test]
    fn rejects_zero_version() {
        let mut data = make_valid_lrf(64);
        data[4..6].copy_from_slice(&0u16.to_le_bytes());
        let evidence = SliceEvidence { data };
        let handler = LrfCarveHandler::new("lrf".to_string(), 0, 1_048_576);
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
        assert!(
            handler
                .process_hit(&hit_at(0), &ctx)
                .expect("process")
                .is_none()
        );
    }

    #[test]
    fn rejects_excessive_version() {
        let mut data = make_valid_lrf(64);
        data[4..6].copy_from_slice(&10_001u16.to_le_bytes());
        let evidence = SliceEvidence { data };
        let handler = LrfCarveHandler::new("lrf".to_string(), 0, 1_048_576);
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
        assert!(
            handler
                .process_hit(&hit_at(0), &ctx)
                .expect("process")
                .is_none()
        );
    }

    #[test]
    fn rejects_zero_root_object_id() {
        let mut data = make_valid_lrf(64);
        data[16..20].copy_from_slice(&0u32.to_le_bytes());
        let evidence = SliceEvidence { data };
        let handler = LrfCarveHandler::new("lrf".to_string(), 0, 1_048_576);
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
        assert!(
            handler
                .process_hit(&hit_at(0), &ctx)
                .expect("process")
                .is_none()
        );
    }

    #[test]
    fn rejects_zero_num_objects() {
        let mut data = make_valid_lrf(64);
        data[20..24].copy_from_slice(&0u32.to_le_bytes());
        let evidence = SliceEvidence { data };
        let handler = LrfCarveHandler::new("lrf".to_string(), 0, 1_048_576);
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
        assert!(
            handler
                .process_hit(&hit_at(0), &ctx)
                .expect("process")
                .is_none()
        );
    }

    #[test]
    fn rejects_excessive_num_objects() {
        let mut data = make_valid_lrf(64);
        data[20..24].copy_from_slice(&100_001u32.to_le_bytes());
        let evidence = SliceEvidence { data };
        let handler = LrfCarveHandler::new("lrf".to_string(), 0, 1_048_576);
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
        assert!(
            handler
                .process_hit(&hit_at(0), &ctx)
                .expect("process")
                .is_none()
        );
    }

    #[test]
    fn rejects_zero_obj_index_offset() {
        let mut data = make_valid_lrf(64);
        data[24..32].copy_from_slice(&0u64.to_le_bytes());
        let evidence = SliceEvidence { data };
        let handler = LrfCarveHandler::new("lrf".to_string(), 0, 1_048_576);
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
        assert!(
            handler
                .process_hit(&hit_at(0), &ctx)
                .expect("process")
                .is_none()
        );
    }

    #[test]
    fn rejects_obj_index_offset_beyond_declared_size() {
        let mut data = make_valid_lrf(64);
        // Object index offset = 100, but declared size = 64
        data[24..32].copy_from_slice(&100u64.to_le_bytes());
        let evidence = SliceEvidence { data };
        let handler = LrfCarveHandler::new("lrf".to_string(), 0, 1_048_576);
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
        assert!(
            handler
                .process_hit(&hit_at(0), &ctx)
                .expect("process")
                .is_none()
        );
    }

    #[test]
    fn rejects_zero_declared_size() {
        let mut data = make_valid_lrf(64);
        data[8..12].copy_from_slice(&0u32.to_le_bytes());
        let evidence = SliceEvidence { data };
        let handler = LrfCarveHandler::new("lrf".to_string(), 0, 1_048_576);
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
        assert!(
            handler
                .process_hit(&hit_at(0), &ctx)
                .expect("process")
                .is_none()
        );
    }

    #[test]
    fn rejects_declared_size_exceeds_max() {
        let data = make_valid_lrf(64);
        let evidence = SliceEvidence { data };
        // max_size = 32, declared = 64 → reject
        let handler = LrfCarveHandler::new("lrf".to_string(), 0, 32);
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
        assert!(
            handler
                .process_hit(&hit_at(0), &ctx)
                .expect("process")
                .is_none()
        );
    }

    #[test]
    fn rejects_random_data_with_magic() {
        // Simulate a false-positive: magic matches but everything else is garbage.
        let mut data = vec![0xFFu8; 128];
        data[0..4].copy_from_slice(&[0x4C, 0x52, 0x46, 0x00]);
        let evidence = SliceEvidence { data };
        let handler = LrfCarveHandler::new("lrf".to_string(), 0, 1_048_576);
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
        // 0xFFFF version = 65535 > MAX_VERSION → rejected
        assert!(
            handler
                .process_hit(&hit_at(0), &ctx)
                .expect("process")
                .is_none()
        );
    }

    #[test]
    fn rejects_header_too_short() {
        let data = vec![0x4C, 0x52, 0x46, 0x00, 0x01, 0x00];
        let evidence = SliceEvidence { data };
        let handler = LrfCarveHandler::new("lrf".to_string(), 0, 1_048_576);
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
        assert!(handler.process_hit(&hit_at(0), &ctx).is_err());
    }
}
