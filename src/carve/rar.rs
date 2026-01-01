use std::fs::File;
use std::io::Write;

use sha2::{Digest, Sha256};

use crate::carve::{
    output_path, write_range, CarveError, CarveHandler, CarvedFile, ExtractionContext,
};
use crate::scanner::NormalizedHit;

const RAR4_MAGIC: [u8; 7] = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00];
const RAR5_MAGIC: [u8; 8] = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x01, 0x00];

const RAR4_HEAD_FILE: u8 = 0x74;
const RAR4_HEAD_END: u8 = 0x7B;

const RAR5_HEAD_END: u64 = 5;

const MAX_RAR5_HEADER_BYTES: u64 = 1024 * 1024;

pub struct RarCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl RarCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for RarCarveHandler {
    fn file_type(&self) -> &str {
        "rar"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        let mut errors = Vec::new();
        let estimate = match estimate_rar_end(ctx, hit.global_offset, self.max_size, &mut errors) {
            Ok(estimate) => estimate,
            Err(CarveError::Invalid(_)) => return Ok(None),
            Err(_) => return Ok(None),
        };

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let mut file = File::create(&full_path)?;
        let mut md5 = md5::Context::new();
        let mut sha256 = Sha256::new();

        let total_end = hit.global_offset + estimate.end;
        let (written, eof_truncated) = write_range(
            ctx,
            hit.global_offset,
            total_end,
            &mut file,
            &mut md5,
            &mut sha256,
        )?;
        let truncated = estimate.truncated || eof_truncated;
        if eof_truncated {
            errors.push("eof before RAR end".to_string());
        }
        file.flush()?;

        if written < self.min_size {
            let _ = std::fs::remove_file(&full_path);
            return Ok(None);
        }

        let md5_hex = format!("{:x}", md5.compute());
        let sha256_hex = hex::encode(sha256.finalize());
        let global_end = if written == 0 {
            hit.global_offset
        } else {
            hit.global_offset + written - 1
        };

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
            validated: !truncated,
            truncated,
            errors,
            pattern_id: Some(hit.pattern_id.clone()),
        }))
    }
}

struct RarEstimate {
    end: u64,
    truncated: bool,
}

fn estimate_rar_end(
    ctx: &ExtractionContext,
    start: u64,
    max_size: u64,
    errors: &mut Vec<String>,
) -> Result<RarEstimate, CarveError> {
    let sig = read_exact_at(ctx, start, RAR5_MAGIC.len()).ok_or(CarveError::Eof)?;
    if sig[..RAR4_MAGIC.len()] == RAR4_MAGIC {
        return parse_rar4(ctx, start, max_size, errors);
    }
    if sig == RAR5_MAGIC {
        return parse_rar5(ctx, start, max_size, errors);
    }
    Err(CarveError::Invalid("rar signature mismatch".to_string()))
}

fn parse_rar4(
    ctx: &ExtractionContext,
    start: u64,
    max_size: u64,
    errors: &mut Vec<String>,
) -> Result<RarEstimate, CarveError> {
    let mut offset = start + RAR4_MAGIC.len() as u64;
    let mut truncated = false;

    loop {
        if max_size > 0 && offset - start >= max_size {
            truncated = true;
            errors.push("max_size reached before RAR end".to_string());
            break;
        }

        let header = match read_exact_at(ctx, offset, 7) {
            Some(buf) => buf,
            None => {
                truncated = true;
                errors.push("eof before RAR end".to_string());
                break;
            }
        };

        let head_type = header[2];
        let flags = u16::from_le_bytes([header[3], header[4]]);
        let head_size = u16::from_le_bytes([header[5], header[6]]) as u64;

        if head_size < 7 {
            return Err(CarveError::Invalid("rar header size too small".to_string()));
        }

        if max_size > 0 && (offset - start).saturating_add(head_size) > max_size {
            truncated = true;
            errors.push("max_size reached before RAR end".to_string());
            break;
        }

        if head_type == RAR4_HEAD_END {
            offset = offset.saturating_add(head_size);
            break;
        }

        if head_type == RAR4_HEAD_FILE {
            let pack_size = match read_exact_at(ctx, offset + 7, 4) {
                Some(buf) => u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as u64,
                None => {
                    truncated = true;
                    errors.push("eof while reading RAR file header".to_string());
                    break;
                }
            };

            let mut pack_size_full = pack_size;
            if flags & 0x0100 != 0 {
                if head_size < 7 + 25 + 4 {
                    return Err(CarveError::Invalid(
                        "rar header missing high pack size".to_string(),
                    ));
                }
                let high = match read_exact_at(ctx, offset + 7 + 25, 4) {
                    Some(buf) => u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as u64,
                    None => {
                        truncated = true;
                        errors.push("eof while reading RAR high pack size".to_string());
                        break;
                    }
                };
                pack_size_full |= high << 32;
            }

            offset = offset
                .saturating_add(head_size)
                .saturating_add(pack_size_full);
        } else {
            offset = offset.saturating_add(head_size);
        }
    }

    Ok(RarEstimate {
        end: offset.saturating_sub(start),
        truncated,
    })
}

fn parse_rar5(
    ctx: &ExtractionContext,
    start: u64,
    max_size: u64,
    errors: &mut Vec<String>,
) -> Result<RarEstimate, CarveError> {
    let mut offset = start + RAR5_MAGIC.len() as u64;
    let mut truncated = false;

    loop {
        if max_size > 0 && offset - start >= max_size {
            truncated = true;
            errors.push("max_size reached before RAR end".to_string());
            break;
        }

        let _crc = match read_exact_at(ctx, offset, 4) {
            Some(buf) => buf,
            None => {
                truncated = true;
                errors.push("eof before RAR end".to_string());
                break;
            }
        };

        let (header_size, size_len) = match read_varint_at(ctx, offset + 4) {
            Some(v) => v,
            None => {
                truncated = true;
                errors.push("eof while reading RAR header size".to_string());
                break;
            }
        };

        if header_size == 0 || header_size > MAX_RAR5_HEADER_BYTES {
            return Err(CarveError::Invalid("rar5 header size invalid".to_string()));
        }

        let header_buf =
            match read_exact_at(ctx, offset + 4 + size_len as u64, header_size as usize) {
                Some(buf) => buf,
                None => {
                    truncated = true;
                    errors.push("eof while reading RAR header".to_string());
                    break;
                }
            };

        let mut idx = 0usize;
        let header_type = read_varint_buf(&header_buf, &mut idx)
            .ok_or_else(|| CarveError::Invalid("rar5 header type missing".to_string()))?;
        let flags = read_varint_buf(&header_buf, &mut idx)
            .ok_or_else(|| CarveError::Invalid("rar5 header flags missing".to_string()))?;

        if flags & 0x01 != 0 {
            let _ = read_varint_buf(&header_buf, &mut idx);
        }
        let data_size = if flags & 0x02 != 0 {
            read_varint_buf(&header_buf, &mut idx).unwrap_or(0)
        } else {
            0
        };

        let header_total = 4u64 + size_len + header_size;
        let block_total = header_total.saturating_add(data_size);

        if max_size > 0 && (offset - start).saturating_add(block_total) > max_size {
            truncated = true;
            errors.push("max_size reached before RAR end".to_string());
            break;
        }

        offset = offset.saturating_add(block_total);

        if header_type == RAR5_HEAD_END {
            break;
        }
    }

    Ok(RarEstimate {
        end: offset.saturating_sub(start),
        truncated,
    })
}

fn read_varint_at(ctx: &ExtractionContext, offset: u64) -> Option<(u64, u64)> {
    let mut value = 0u64;
    let mut shift = 0u32;
    let mut idx = 0u64;
    while idx < 10 {
        let byte = read_exact_at(ctx, offset + idx, 1)?[0];
        value |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some((value, idx + 1));
        }
        shift += 7;
        idx += 1;
    }
    None
}

fn read_varint_buf(buf: &[u8], idx: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    let mut shift = 0u32;
    let mut read = 0u32;
    while *idx < buf.len() && read < 10 {
        let byte = buf[*idx];
        *idx += 1;
        read += 1;
        value |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
        if shift > 63 {
            break;
        }
    }
    None
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
    use super::RarCarveHandler;
    use crate::carve::{CarveHandler, ExtractionContext};
    use crate::evidence::RawFileSource;
    use crate::scanner::NormalizedHit;

    #[test]
    fn carves_minimal_rar4() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let mut rar = Vec::new();
        rar.extend_from_slice(&super::RAR4_MAGIC);
        rar.extend_from_slice(&[0u8; 2]); // crc
        rar.push(0x73); // main header
        rar.extend_from_slice(&0u16.to_le_bytes());
        rar.extend_from_slice(&13u16.to_le_bytes());
        rar.extend_from_slice(&[0u8; 6]);
        rar.extend_from_slice(&[0u8; 2]); // crc
        rar.push(super::RAR4_HEAD_END);
        rar.extend_from_slice(&0u16.to_le_bytes());
        rar.extend_from_slice(&7u16.to_le_bytes());

        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &rar).expect("write rar");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
        };
        let handler = RarCarveHandler::new("rar".to_string(), 8, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "rar".to_string(),
            pattern_id: "rar_header".to_string(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        let carved = carved.expect("carved");
        assert!(carved.validated);
        assert_eq!(carved.size, rar.len() as u64);
    }

    #[test]
    fn carves_minimal_rar5() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let mut rar = Vec::new();
        rar.extend_from_slice(&super::RAR5_MAGIC);
        rar.extend_from_slice(&[0u8; 4]); // crc
        rar.push(2); // header size
        rar.push(1); // type main
        rar.push(0); // flags
        rar.extend_from_slice(&[0u8; 4]); // crc
        rar.push(2); // header size
        rar.push(super::RAR5_HEAD_END as u8); // type end
        rar.push(0); // flags

        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &rar).expect("write rar");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
        };
        let handler = RarCarveHandler::new("rar".to_string(), 8, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "rar".to_string(),
            pattern_id: "rar5_header".to_string(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        let carved = carved.expect("carved");
        assert!(carved.validated);
        assert_eq!(carved.size, rar.len() as u64);
    }
}
