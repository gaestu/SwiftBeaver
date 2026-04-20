use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, PendingCarve, PreValidation,
    create_hashers, finalize_hashers, output_path, write_range,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const BOX_HEADER_LEN: usize = 8;
const EXTENDED_HEADER_LEN: usize = 16;

pub struct Mp4CarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
    allow_quicktime: bool,
}

impl Mp4CarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64, allow_quicktime: bool) -> Self {
        Self {
            extension,
            min_size,
            max_size,
            allow_quicktime,
        }
    }
}

impl CarveHandler for Mp4CarveHandler {
    fn file_type(&self) -> &str {
        "mp4"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let mut buf = [0u8; 12];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < buf.len() {
            return Ok(PreValidation::Reject("truncated header".to_string()));
        }
        if &buf[4..8] != b"ftyp" {
            return Ok(PreValidation::Reject(
                "mp4 ftyp marker mismatch".to_string(),
            ));
        }
        let size32 = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
        if size32 <= 7 || size32 > 1024 * 1024 {
            return Ok(PreValidation::Reject(
                "mp4 ftyp box size implausible".to_string(),
            ));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
        let mut errors = Vec::new();
        let mut truncated = false;
        let mut seen_ftyp = false;
        let mut seen_moov = false;

        let mut offset = hit.global_offset;
        let mut last_good = hit.global_offset;

        loop {
            if self.max_size > 0 && offset - hit.global_offset >= self.max_size {
                truncated = true;
                errors.push("max_size reached before MP4 end".to_string());
                break;
            }

            let header = match read_exact_at(ctx, offset, BOX_HEADER_LEN) {
                Some(buf) => buf,
                None => {
                    let evidence_len = ctx.evidence.len();
                    if seen_ftyp
                        && seen_moov
                        && offset.saturating_add(BOX_HEADER_LEN as u64) > evidence_len
                    {
                        break;
                    }
                    truncated = true;
                    errors.push("eof before MP4 end".to_string());
                    break;
                }
            };

            let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
            let box_type = &header[4..8];

            let (box_size, header_len) = if size32 == 1 {
                let ext = match read_exact_at(ctx, offset, EXTENDED_HEADER_LEN) {
                    Some(buf) => buf,
                    None => {
                        if seen_ftyp && seen_moov {
                            break;
                        }
                        truncated = true;
                        errors.push("eof before MP4 extended size".to_string());
                        break;
                    }
                };
                let size64 = u64::from_be_bytes([
                    ext[8], ext[9], ext[10], ext[11], ext[12], ext[13], ext[14], ext[15],
                ]);
                (size64, EXTENDED_HEADER_LEN as u64)
            } else if size32 == 0 {
                if seen_ftyp && seen_moov {
                    break;
                }
                truncated = true;
                errors.push("mp4 box size 0 encountered".to_string());
                break;
            } else {
                (size32, BOX_HEADER_LEN as u64)
            };

            if box_size < header_len || box_size == 0 {
                if seen_ftyp && seen_moov {
                    break;
                }
                return Ok(None);
            }

            if offset == hit.global_offset {
                if box_type != b"ftyp" {
                    return Ok(None);
                }
                if let Some(brand) = read_exact_at(ctx, offset.saturating_add(header_len), 4)
                    && brand == b"qt  "
                    && !self.allow_quicktime
                {
                    return Ok(None);
                }
                seen_ftyp = true;
            }

            if box_type == b"moov" {
                seen_moov = true;
            }

            if self.max_size > 0
                && (offset - hit.global_offset).saturating_add(box_size) > self.max_size
            {
                truncated = true;
                errors.push("max_size reached before MP4 end".to_string());
                break;
            }

            offset = offset.saturating_add(box_size);
            last_good = offset;
        }

        if !seen_ftyp || !seen_moov {
            return Ok(None);
        }

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let (mut md5, mut sha256) = create_hashers(&ctx.hash_config);

        let mut total_end = last_good;
        if self.max_size > 0 && total_end - hit.global_offset > self.max_size {
            total_end = hit.global_offset + self.max_size;
        }

        let (written, eof_truncated, mut writer) = write_range(
            ctx,
            hit.global_offset,
            total_end,
            &full_path,
            md5.as_mut(),
            sha256.as_mut(),
        )?;
        if eof_truncated {
            truncated = true;
            errors.push("eof before MP4 end".to_string());
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
                validated: !truncated,
                truncated,
                errors,
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

#[cfg(test)]
mod tests {
    use super::Mp4CarveHandler;
    use crate::carve::{CarveHandler, ExtractionContext};
    use crate::evidence::RawFileSource;
    use crate::scanner::NormalizedHit;

    #[test]
    fn carves_minimal_mp4() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let mut mp4 = Vec::new();
        mp4.extend_from_slice(&24u32.to_be_bytes());
        mp4.extend_from_slice(b"ftyp");
        mp4.extend_from_slice(b"isom");
        mp4.extend_from_slice(&0u32.to_be_bytes());
        mp4.extend_from_slice(b"isom");
        mp4.extend_from_slice(b"iso2");
        mp4.extend_from_slice(&8u32.to_be_bytes());
        mp4.extend_from_slice(b"moov");

        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &mp4).expect("write mp4");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };
        let handler = Mp4CarveHandler::new("mp4".to_string(), 8, 0, false);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "mp4".to_string(),
            pattern_id: "mp4_ftyp_18".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        let carved = carved.expect("carved").flush().expect("flush");
        assert!(carved.validated);
        assert_eq!(carved.size, mp4.len() as u64);
    }

    #[test]
    fn rejects_quicktime_when_disabled() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let mut mov = Vec::new();
        mov.extend_from_slice(&24u32.to_be_bytes());
        mov.extend_from_slice(b"ftyp");
        mov.extend_from_slice(b"qt  ");
        mov.extend_from_slice(&0u32.to_be_bytes());
        mov.extend_from_slice(b"qt  ");
        mov.extend_from_slice(b"qt  ");
        mov.extend_from_slice(&8u32.to_be_bytes());
        mov.extend_from_slice(b"moov");

        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &mov).expect("write mov");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };
        let handler = Mp4CarveHandler::new("mp4".to_string(), 8, 0, false);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "mp4".to_string(),
            pattern_id: "mp4_ftyp_18".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        assert!(carved.is_none());
    }

    #[test]
    fn carves_quicktime_when_enabled() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let mut mov = Vec::new();
        mov.extend_from_slice(&24u32.to_be_bytes());
        mov.extend_from_slice(b"ftyp");
        mov.extend_from_slice(b"qt  ");
        mov.extend_from_slice(&0u32.to_be_bytes());
        mov.extend_from_slice(b"qt  ");
        mov.extend_from_slice(b"qt  ");
        mov.extend_from_slice(&8u32.to_be_bytes());
        mov.extend_from_slice(b"moov");

        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &mov).expect("write mov");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
            metadata_only: false,
            hash_config: crate::hash::HashConfig::default(),
        };
        let handler = Mp4CarveHandler::new("mp4".to_string(), 8, 0, true);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "mp4".to_string(),
            pattern_id: "mp4_ftyp_18".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        let carved = carved.expect("carved").flush().expect("flush");
        assert!(carved.validated);
        assert_eq!(carved.size, mov.len() as u64);
    }
}
