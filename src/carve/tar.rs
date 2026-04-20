//! TAR archive carving handler.
//!
//! TAR archives consist of 512-byte headers followed by file data.
//! The archive ends with two consecutive zero blocks.

use crate::carve::{
    CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext, PendingCarve,
    PreValidation, output_path,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const TAR_BLOCK_SIZE: usize = 512;
const TAR_USTAR_OFFSET: usize = 257;
const TAR_USTAR_MAGIC: &[u8; 5] = b"ustar";

pub struct TarCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl TarCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for TarCarveHandler {
    fn file_type(&self) -> &str {
        "tar"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let Some(start_offset) = offset.checked_sub(TAR_USTAR_OFFSET as u64) else {
            return Ok(PreValidation::Reject(
                "tar ustar hit before header start".to_string(),
            ));
        };

        let mut header = [0u8; TAR_BLOCK_SIZE];
        let n = evidence
            .read_at(start_offset, &mut header)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < header.len() {
            return Ok(PreValidation::Reject("tar header truncated".to_string()));
        }
        if header[TAR_USTAR_OFFSET..TAR_USTAR_OFFSET + TAR_USTAR_MAGIC.len()] != *TAR_USTAR_MAGIC {
            return Ok(PreValidation::Reject(
                "tar ustar magic mismatch".to_string(),
            ));
        }
        match validate_checksum(&header) {
            Ok(true) => Ok(PreValidation::Proceed),
            Ok(false) => Ok(PreValidation::Reject("tar checksum invalid".to_string())),
            Err(err) => Ok(PreValidation::Reject(err.to_string())),
        }
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
        let start_offset = if hit.pattern_id == "tar_ustar" {
            if hit.global_offset < TAR_USTAR_OFFSET as u64 {
                return Ok(None);
            }
            hit.global_offset - TAR_USTAR_OFFSET as u64
        } else {
            hit.global_offset
        };

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            start_offset,
        )?;
        let mut stream = CarveStream::new(ctx, start_offset, self.max_size, full_path.clone());

        let mut validated = false;
        let mut truncated = false;
        let mut errors = Vec::new();

        let result: Result<u64, CarveError> = (|| {
            let mut zero_blocks = 0u8;
            loop {
                let header = stream.read_exact(TAR_BLOCK_SIZE)?;
                if is_zero_block(&header) {
                    zero_blocks += 1;
                    if zero_blocks >= 2 {
                        validated = true;
                        break;
                    }
                    continue;
                }
                zero_blocks = 0;

                if hit.pattern_id == "tar_ustar"
                    && header[TAR_USTAR_OFFSET..TAR_USTAR_OFFSET + TAR_USTAR_MAGIC.len()]
                        != *TAR_USTAR_MAGIC
                {
                    return Err(CarveError::Invalid("tar ustar magic mismatch".to_string()));
                }

                if !validate_checksum(&header)? {
                    return Err(CarveError::Invalid("tar checksum invalid".to_string()));
                }

                let size = parse_octal(&header[124..136])?;
                let blocks = size.div_ceil(TAR_BLOCK_SIZE as u64);
                let data_len = blocks.saturating_mul(TAR_BLOCK_SIZE as u64);
                if data_len > 0 {
                    stream.read_exact(data_len as usize)?;
                }
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

        let (size, md5_hex, sha256_hex, mut writer) = stream.finalize()?;

        if size < self.min_size {
            writer.discard();
            return Ok(None);
        }

        if self.max_size > 0 && size >= self.max_size {
            truncated = true;
            if !errors.iter().any(|e| e.contains("max_size")) {
                errors.push("max_size reached".to_string());
            }
        }

        let global_end = if size == 0 {
            start_offset
        } else {
            start_offset + size - 1
        };

        Ok(Some(PendingCarve::new(
            CarvedFile {
                run_id: ctx.run_id.to_string(),
                file_type: self.file_type().to_string(),
                path: rel_path,
                extension: self.extension.clone(),
                global_start: start_offset,
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
            },
            writer,
        )))
    }
}

fn is_zero_block(block: &[u8]) -> bool {
    block.iter().all(|b| *b == 0)
}

fn parse_octal(field: &[u8]) -> Result<u64, CarveError> {
    let mut value = 0u64;
    let mut seen = false;
    for &b in field {
        if b == 0 || b == b' ' || b == b'\n' {
            if seen {
                break;
            }
            continue;
        }
        if !(b'0'..=b'7').contains(&b) {
            return Err(CarveError::Invalid("tar octal field invalid".to_string()));
        }
        seen = true;
        value = value.saturating_mul(8).saturating_add((b - b'0') as u64);
    }
    Ok(value)
}

fn validate_checksum(header: &[u8]) -> Result<bool, CarveError> {
    if header.len() < TAR_BLOCK_SIZE {
        return Err(CarveError::Invalid("tar header too short".to_string()));
    }
    let stored = parse_octal(&header[148..156])? as u32;
    let mut sum = 0u32;
    for (idx, &b) in header.iter().enumerate() {
        if (148..156).contains(&idx) {
            sum = sum.saturating_add(0x20);
        } else {
            sum = sum.saturating_add(b as u32);
        }
    }
    Ok(sum == stored)
}

#[cfg(test)]
mod tests {
    use super::TarCarveHandler;
    use crate::carve::{CarveHandler, ExtractionContext, PreValidation};
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

    fn build_minimal_tar() -> Vec<u8> {
        let mut header = vec![0u8; 512];
        header[0..8].copy_from_slice(b"file.txt");
        header[100..108].copy_from_slice(b"0000777\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        header[124..136].copy_from_slice(b"00000000000\0");
        header[136..148].copy_from_slice(b"00000000000\0");
        header[257..262].copy_from_slice(b"ustar");
        header[262..264].copy_from_slice(b"00");

        let mut sum = 0u32;
        for (idx, &b) in header.iter().enumerate() {
            if (148..156).contains(&idx) {
                sum = sum.saturating_add(0x20);
            } else {
                sum = sum.saturating_add(b as u32);
            }
        }
        let checksum = format!("{:06o}\0 ", sum);
        header[148..156].copy_from_slice(checksum.as_bytes());

        let mut tar = Vec::new();
        tar.extend_from_slice(&header);
        tar.extend_from_slice(&[0u8; 512]);
        tar.extend_from_slice(&[0u8; 512]);
        tar
    }

    #[test]
    fn carves_minimal_tar_from_ustar() {
        let tar_data = build_minimal_tar();
        let evidence = SliceEvidence {
            data: tar_data.clone(),
        };
        let handler = TarCarveHandler::new("tar".to_string(), 0, 0);
        let hit = NormalizedHit {
            global_offset: 257,
            file_type_id: "tar".to_string(),
            pattern_id: "tar_ustar".to_string(),
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

        let carved = handler.process_hit(&hit, &ctx).expect("process");
        let carved = carved.expect("carved").flush().expect("flush");
        assert!(carved.validated);
        assert_eq!(carved.size, tar_data.len() as u64);
        assert_eq!(carved.global_start, 0);
    }

    #[test]
    fn pre_validate_accepts_valid_tar_ustar_hit() {
        let tar_data = build_minimal_tar();
        let evidence = SliceEvidence { data: tar_data };
        let handler = TarCarveHandler::new("tar".to_string(), 0, 0);

        assert_eq!(
            handler.pre_validate(&evidence, 257).expect("pre_validate"),
            PreValidation::Proceed
        );
    }

    #[test]
    fn pre_validate_rejects_invalid_tar_checksum() {
        let mut tar_data = build_minimal_tar();
        tar_data[148] = b'9';
        let evidence = SliceEvidence { data: tar_data };
        let handler = TarCarveHandler::new("tar".to_string(), 0, 0);

        assert!(
            matches!(
                handler.pre_validate(&evidence, 257).expect("pre_validate"),
                PreValidation::Reject(_)
            ),
            "invalid tar headers should be rejected during pre-validation"
        );
    }
}
