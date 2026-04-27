//! ICO/CUR carving handler.
//!
//! ICO files have a small header with directory entries containing offsets/sizes.
//! Enhanced validation verifies that at least one entry contains valid BMP or PNG data.

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, PendingCarve, PreValidation,
    create_hashers, finalize_hashers, output_path, write_range,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

/// PNG signature at start of image data within ICO
const PNG_HEADER_MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

const ICONDIR_LEN: usize = 6;
const ICONDIRENTRY_LEN: usize = 16;

/// Maximum reasonable icon entries (Windows typically uses 1-10)
const MAX_ICON_ENTRIES: usize = 64;
/// Maximum reasonable single icon image size (256x256 @ 32bpp + overhead)
const MAX_SINGLE_IMAGE_SIZE: u64 = 512 * 1024; // 512 KB per image
/// Maximum reasonable total ICO size
const MAX_REASONABLE_ICO_SIZE: u64 = 4 * 1024 * 1024; // 4 MB total

#[derive(Debug, Clone, Copy)]
struct IconResource {
    size: u64,
    offset: u64,
}

pub struct IcoCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl IcoCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }

    fn effective_total_max_size(&self) -> u64 {
        if self.max_size > 0 {
            self.max_size.min(MAX_REASONABLE_ICO_SIZE)
        } else {
            MAX_REASONABLE_ICO_SIZE
        }
    }

    /// Validate that data at the given offset looks like valid BMP or PNG image data
    fn validate_image_data(
        ctx: &ExtractionContext,
        offset: u64,
        size: u64,
    ) -> Result<bool, CarveError> {
        if size < 8 {
            return Ok(false);
        }
        let header = match read_exact_at(ctx, offset, 8)? {
            Some(h) => h,
            None => return Ok(false),
        };

        // Check for PNG signature (embedded PNG in ICO)
        if header.starts_with(&PNG_HEADER_MAGIC) {
            return Ok(true);
        }

        if size < 16 {
            return Ok(false);
        }

        let dib_header = match read_exact_at(ctx, offset, 16)? {
            Some(h) => h,
            None => return Ok(false),
        };

        let dib_size =
            u32::from_le_bytes([dib_header[0], dib_header[1], dib_header[2], dib_header[3]]) as u64;
        if !matches!(dib_size, 40 | 52 | 56 | 108 | 124) || dib_size > size {
            return Ok(false);
        }

        let width =
            i32::from_le_bytes([dib_header[4], dib_header[5], dib_header[6], dib_header[7]]);
        let height =
            i32::from_le_bytes([dib_header[8], dib_header[9], dib_header[10], dib_header[11]]);
        let planes = u16::from_le_bytes([dib_header[12], dib_header[13]]);
        let bit_count = u16::from_le_bytes([dib_header[14], dib_header[15]]);

        // ICO DIBs must use positive width/height. Top-down DIBs (negative
        // height) are not used inside ICO containers. Height is the doubled
        // value of the icon height (XOR mask + AND mask), so a 256-pixel-tall
        // icon reports height = 512; the bound below accommodates that and
        // rejects clearly implausible values.
        if width <= 0 || width > 256 {
            return Ok(false);
        }
        if height <= 0 || height > 512 {
            return Ok(false);
        }
        if planes != 1 {
            return Ok(false);
        }

        Ok(matches!(bit_count, 1 | 4 | 8 | 16 | 24 | 32))
    }

    fn parse_resources(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
        count: usize,
    ) -> Result<Option<(Vec<IconResource>, u64)>, CarveError> {
        let dir_len = match count.checked_mul(ICONDIRENTRY_LEN) {
            Some(len) => len,
            None => return Ok(None),
        };
        let header_size = ICONDIR_LEN as u64 + dir_len as u64;
        let max_total_size = self.effective_total_max_size();
        if header_size > max_total_size {
            return Ok(None);
        }

        let dir_offset = match offset.checked_add(ICONDIR_LEN as u64) {
            Some(value) => value,
            None => return Ok(None),
        };
        let dir = match read_exact_from_evidence(evidence, dir_offset, dir_len)? {
            Some(bytes) => bytes,
            None => return Ok(None),
        };

        let mut resources = Vec::with_capacity(count);
        let mut max_end = header_size;

        for entry_index in 0..count {
            let base = entry_index * ICONDIRENTRY_LEN;
            let size =
                u32::from_le_bytes([dir[base + 8], dir[base + 9], dir[base + 10], dir[base + 11]])
                    as u64;
            let image_offset = u32::from_le_bytes([
                dir[base + 12],
                dir[base + 13],
                dir[base + 14],
                dir[base + 15],
            ]) as u64;

            if size == 0 || image_offset < header_size {
                return Ok(None);
            }
            if size > MAX_SINGLE_IMAGE_SIZE || size > max_total_size {
                return Ok(None);
            }

            let end = match image_offset.checked_add(size) {
                Some(value) => value,
                None => return Ok(None),
            };
            if end > max_total_size {
                return Ok(None);
            }

            max_end = max_end.max(end);
            resources.push(IconResource {
                size,
                offset: image_offset,
            });
        }

        Ok(Some((resources, max_end)))
    }
}

impl CarveHandler for IcoCarveHandler {
    fn file_type(&self) -> &str {
        "ico"
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
        let mut buf = [0u8; ICONDIR_LEN];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < buf.len() {
            return Ok(PreValidation::Reject("truncated header".to_string()));
        }
        if buf[0] != 0x00 || buf[1] != 0x00 {
            return Ok(PreValidation::Reject(
                "ico reserved bytes invalid".to_string(),
            ));
        }
        let icon_type = u16::from_le_bytes([buf[2], buf[3]]);
        if icon_type != 1 && icon_type != 2 {
            return Ok(PreValidation::Reject("ico type invalid".to_string()));
        }
        let count = u16::from_le_bytes([buf[4], buf[5]]);
        if count == 0 || count as usize > MAX_ICON_ENTRIES {
            return Ok(PreValidation::Reject(
                "ico image count implausible".to_string(),
            ));
        }

        // Keep pre_validate light: full directory parsing and per-entry
        // payload validation happen in process_hit, which re-reads and
        // re-validates everything anyway.
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
        let header = read_exact_at(ctx, hit.global_offset, ICONDIR_LEN)?
            .ok_or_else(|| CarveError::Invalid("ico header too short".to_string()))?;
        if header[0] != 0 || header[1] != 0 {
            return Ok(None);
        }
        let icon_type = u16::from_le_bytes([header[2], header[3]]);
        if icon_type != 1 && icon_type != 2 {
            return Ok(None);
        }
        let count = u16::from_le_bytes([header[4], header[5]]) as usize;
        if count == 0 || count > MAX_ICON_ENTRIES {
            return Ok(None);
        }

        let Some((resources, max_end)) =
            self.parse_resources(ctx.evidence, hit.global_offset, count)?
        else {
            return Ok(None);
        };

        for resource in &resources {
            let image_global_offset = match hit.global_offset.checked_add(resource.offset) {
                Some(value) => value,
                None => return Ok(None),
            };
            if !Self::validate_image_data(ctx, image_global_offset, resource.size)? {
                return Ok(None);
            }
        }

        let total_end = match hit.global_offset.checked_add(max_end) {
            Some(value) => value,
            None => return Ok(None),
        };

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
        let mut errors = Vec::new();
        if eof_truncated {
            errors.push("eof before ICO end".to_string());
        }

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
                validated: !eof_truncated,
                truncated: eof_truncated,
                errors,
                pattern_id: Some(hit.pattern_id.clone()),
                is_duplicate: false,
                duplicate_of_offset: None,
            },
            writer,
        )))
    }
}

fn read_exact_at(
    ctx: &ExtractionContext,
    offset: u64,
    len: usize,
) -> Result<Option<Vec<u8>>, CarveError> {
    read_exact_from_evidence(ctx.evidence, offset, len)
}

fn read_exact_from_evidence(
    evidence: &dyn EvidenceSource,
    offset: u64,
    len: usize,
) -> Result<Option<Vec<u8>>, CarveError> {
    let mut buf = vec![0u8; len];
    let n = evidence
        .read_at(offset, &mut buf)
        .map_err(|e| CarveError::Evidence(e.to_string()))?;
    if n < len {
        return Ok(None);
    }
    Ok(Some(buf))
}

#[cfg(test)]
mod tests {
    use super::{ICONDIR_LEN, ICONDIRENTRY_LEN, IcoCarveHandler, PNG_HEADER_MAGIC};
    use crate::carve::{CarveHandler, CarvedFile, ExtractionContext, PreValidation};
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

    struct TestEntry {
        payload: Vec<u8>,
        offset: u32,
        declared_size: u32,
    }

    fn bmp_payload(width: i32, height: i32) -> Vec<u8> {
        dib_payload(40, width, height)
    }

    fn dib_payload(dib_size: u32, width: i32, height: i32) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&dib_size.to_le_bytes());
        payload.extend_from_slice(&width.to_le_bytes());
        payload.extend_from_slice(&(height * 2).to_le_bytes());
        payload.extend_from_slice(&1u16.to_le_bytes());
        payload.extend_from_slice(&32u16.to_le_bytes());
        payload.resize(dib_size as usize, 0);
        payload.extend_from_slice(&[0xAA; 4]);
        payload
    }

    fn png_payload() -> Vec<u8> {
        let mut payload = PNG_HEADER_MAGIC.to_vec();
        payload.extend_from_slice(&[0xBB; 8]);
        payload
    }

    fn build_icon_from_entries(icon_type: u16, entries: &[TestEntry]) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&icon_type.to_le_bytes());
        data.extend_from_slice(&(entries.len() as u16).to_le_bytes());

        for entry in entries {
            data.extend_from_slice(&[16, 16, 0, 0]);
            data.extend_from_slice(&1u16.to_le_bytes());
            data.extend_from_slice(&32u16.to_le_bytes());
            data.extend_from_slice(&entry.declared_size.to_le_bytes());
            data.extend_from_slice(&entry.offset.to_le_bytes());
        }

        let mut entries_by_offset = entries.iter().collect::<Vec<_>>();
        entries_by_offset.sort_by_key(|entry| entry.offset);
        for entry in entries_by_offset {
            let image_offset = entry.offset as usize;
            assert!(data.len() <= image_offset, "test entries must not overlap");
            data.resize(image_offset, 0);
            data.extend_from_slice(&entry.payload);
        }

        data
    }

    fn build_sequential_icon(icon_type: u16, payloads: Vec<Vec<u8>>) -> Vec<u8> {
        let header_size = ICONDIR_LEN + payloads.len() * ICONDIRENTRY_LEN;
        let mut offset = header_size as u32;
        let mut entries = Vec::new();
        for payload in payloads {
            let declared_size = payload.len() as u32;
            entries.push(TestEntry {
                payload,
                offset,
                declared_size,
            });
            offset += declared_size;
        }
        build_icon_from_entries(icon_type, &entries)
    }

    fn carve(data: Vec<u8>, extension: &str, pattern_id: &str) -> Option<CarvedFile> {
        let evidence = SliceEvidence { data };
        let handler = IcoCarveHandler::new(extension.to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "ico".to_string(),
            pattern_id: pattern_id.to_string(),
            chunk_data: None,
            chunk_start: 0,
        };
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

        handler
            .process_hit(&hit, &ctx)
            .expect("process")
            .map(|pending| pending.flush().expect("flush"))
    }

    #[test]
    fn carves_minimal_ico_to_declared_size_without_trailing_junk() {
        let mut data = build_sequential_icon(1, vec![bmp_payload(16, 16)]);
        let expected_size = data.len() as u64;
        data.extend_from_slice(b"trailing bytes from evidence");

        let carved = carve(data, "ico", "ico_header").expect("carved");
        assert_eq!(carved.size, expected_size);
        assert!(carved.validated);
        assert!(!carved.truncated);
    }

    #[test]
    fn carves_mixed_bmp_png_to_max_declared_end() {
        let header_size = ICONDIR_LEN + 2 * ICONDIRENTRY_LEN;
        let png = png_payload();
        let bmp = bmp_payload(16, 16);
        let entries = vec![
            TestEntry {
                offset: (header_size + 40) as u32,
                declared_size: bmp.len() as u32,
                payload: bmp,
            },
            TestEntry {
                offset: header_size as u32,
                declared_size: png.len() as u32,
                payload: png,
            },
        ];
        let expected_size = entries
            .iter()
            .map(|entry| entry.offset as u64 + entry.declared_size as u64)
            .max()
            .expect("max end");
        let data = build_icon_from_entries(1, &entries);

        let carved = carve(data, "ico", "ico_header").expect("carved");
        assert_eq!(carved.size, expected_size);
        assert!(carved.validated);
    }

    #[test]
    fn accepts_cur_container() {
        let data = build_sequential_icon(2, vec![bmp_payload(16, 16)]);

        let carved = carve(data, "cur", "cur_header").expect("carved");
        assert!(carved.validated);
        assert_eq!(carved.pattern_id.as_deref(), Some("cur_header"));
    }

    #[test]
    fn accepts_extended_dib_header_size() {
        let data = build_sequential_icon(1, vec![dib_payload(108, 16, 16)]);

        let carved = carve(data, "ico", "ico_header").expect("carved");
        assert!(carved.validated);
    }

    #[test]
    fn rejects_implausible_count_in_prevalidation() {
        let data = [0x00, 0x00, 0x01, 0x00, 0xFF, 0xFF];
        let evidence = SliceEvidence {
            data: data.to_vec(),
        };
        let handler = IcoCarveHandler::new("ico".to_string(), 0, 0);

        let result = handler.pre_validate(&evidence, 0).expect("pre-validate");
        assert!(matches!(result, PreValidation::Reject(_)));
    }

    #[test]
    fn rejects_image_offset_inside_directory() {
        let payload = bmp_payload(16, 16);
        let mut data = Vec::new();
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&[16, 16, 0, 0]);
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&32u16.to_le_bytes());
        data.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&payload);

        let carved = carve(data, "ico", "ico_header");
        assert!(carved.is_none());
    }

    #[test]
    fn rejects_declared_extent_over_effective_max() {
        // Use a small per-image size that passes MAX_SINGLE_IMAGE_SIZE (512 KiB)
        // and the per-entry total cap, but place image_offset such that
        // image_offset + size exceeds effective_total_max_size, exercising the
        // overflow-end branch in parse_resources.
        let payload = bmp_payload(16, 16);
        let evidence_max = 100 * 1024u64; // configured max -> effective cap = 100 KiB
        let near_cap_offset = (evidence_max - 1024) as u32; // 99 KiB
        let declared_size = 8 * 1024u32; // 8 KiB; end = 107 KiB > 100 KiB
        let data = build_icon_from_entries(
            1,
            &[TestEntry {
                offset: near_cap_offset,
                declared_size,
                payload,
            }],
        );

        let evidence = SliceEvidence { data };
        let handler = IcoCarveHandler::new("ico".to_string(), 0, evidence_max);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "ico".to_string(),
            pattern_id: "ico_header".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };
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

        let result = handler.process_hit(&hit, &ctx).expect("process");
        assert!(result.is_none());
    }

    #[test]
    fn flags_truncated_when_resource_extends_past_evidence() {
        let payload = bmp_payload(16, 16);
        let data = build_icon_from_entries(
            1,
            &[TestEntry {
                offset: (ICONDIR_LEN + ICONDIRENTRY_LEN) as u32,
                declared_size: payload.len() as u32 + 100,
                payload,
            }],
        );

        let carved = carve(data, "ico", "ico_header").expect("carved");
        assert!(!carved.validated);
        assert!(carved.truncated);
        assert_eq!(carved.errors, vec!["eof before ICO end".to_string()]);
    }

    #[test]
    fn rejects_when_any_declared_entry_has_invalid_payload() {
        let data = build_sequential_icon(1, vec![bmp_payload(16, 16), vec![0xCC; 16]]);

        let carved = carve(data, "ico", "ico_header");
        assert!(carved.is_none());
    }
}
