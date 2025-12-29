use std::collections::{HashSet, VecDeque};
use std::fs::File;
use std::io::Write;

use sha2::{Digest, Sha256};

use crate::carve::{output_path, write_range, CarveError, CarveHandler, CarvedFile, ExtractionContext};
use crate::scanner::NormalizedHit;

const TIFF_HEADER_LEN: usize = 8;
const MAX_IFD_ENTRIES: u16 = 4096;
const MAX_TIFF_ARRAY_ENTRIES: u64 = 1_000_000;
const MAX_TIFF_DATA_BYTES: u64 = 16 * 1024 * 1024;

const TAG_STRIP_OFFSETS: u16 = 273;
const TAG_STRIP_BYTE_COUNTS: u16 = 279;
const TAG_TILE_OFFSETS: u16 = 324;
const TAG_TILE_BYTE_COUNTS: u16 = 325;
const TAG_SUB_IFD: u16 = 330;
const TAG_EXIF_IFD: u16 = 34665;
const TAG_GPS_IFD: u16 = 34853;

#[derive(Debug, Clone, Copy)]
enum Endian {
    Little,
    Big,
}

pub struct TiffCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl TiffCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for TiffCarveHandler {
    fn file_type(&self) -> &str {
        "tiff"
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
        let estimate = match estimate_tiff_end(ctx, hit.global_offset, &mut errors) {
            Ok(estimate) => estimate,
            Err(CarveError::Invalid(_)) => return Ok(None),
            Err(_) => return Ok(None),
        };
        if estimate.end <= 0 {
            return Ok(None);
        }

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let mut file = File::create(&full_path)?;
        let mut md5 = md5::Context::new();
        let mut sha256 = Sha256::new();

        let mut total_end = hit.global_offset + estimate.end;
        let mut truncated = estimate.truncated;
        if self.max_size > 0 && estimate.end > self.max_size {
            total_end = hit.global_offset + self.max_size;
            truncated = true;
            errors.push("max_size reached before TIFF end".to_string());
        }

        let (written, eof_truncated) =
            write_range(ctx, hit.global_offset, total_end, &mut file, &mut md5, &mut sha256)?;
        if eof_truncated {
            truncated = true;
            errors.push("eof before TIFF end".to_string());
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

struct TiffEstimate {
    end: u64,
    truncated: bool,
}

fn estimate_tiff_end(
    ctx: &ExtractionContext,
    start: u64,
    errors: &mut Vec<String>,
) -> Result<TiffEstimate, CarveError> {
    let header = read_exact_at(ctx, start, TIFF_HEADER_LEN).ok_or(CarveError::Eof)?;
    let endian = match &header[0..4] {
        [0x49, 0x49, 0x2A, 0x00] => Endian::Little,
        [0x4D, 0x4D, 0x00, 0x2A] => Endian::Big,
        _ => return Err(CarveError::Invalid("tiff signature mismatch".to_string())),
    };

    let first_ifd_offset = read_u32(&header[4..8], endian) as u64;
    let mut max_end = TIFF_HEADER_LEN as u64;
    let mut truncated = false;

    let mut queue = VecDeque::new();
    if first_ifd_offset >= TIFF_HEADER_LEN as u64 {
        queue.push_back(first_ifd_offset);
    }

    let mut seen = HashSet::new();
    while let Some(ifd_offset) = queue.pop_front() {
        if ifd_offset == 0 || !seen.insert(ifd_offset) {
            continue;
        }
        match parse_ifd(ctx, start, ifd_offset, endian, &mut max_end, &mut queue) {
            Ok(()) => {}
            Err(CarveError::Eof) => {
                truncated = true;
                errors.push("eof while reading TIFF IFD".to_string());
                break;
            }
            Err(CarveError::Invalid(msg)) => {
                errors.push(msg);
                truncated = true;
                break;
            }
            Err(other) => return Err(other),
        }
    }

    Ok(TiffEstimate {
        end: max_end,
        truncated,
    })
}

fn parse_ifd(
    ctx: &ExtractionContext,
    start: u64,
    ifd_offset: u64,
    endian: Endian,
    max_end: &mut u64,
    queue: &mut VecDeque<u64>,
) -> Result<(), CarveError> {
    let base = start.saturating_add(ifd_offset);
    let count_buf = read_exact_at(ctx, base, 2).ok_or(CarveError::Eof)?;
    let count = read_u16(&count_buf, endian);
    if count > MAX_IFD_ENTRIES {
        return Err(CarveError::Invalid("tiff IFD entry count too large".to_string()));
    }
    let entries_len = count as usize * 12;
    let total_len = 2 + entries_len + 4;
    let ifd_buf = read_exact_at(ctx, base, total_len).ok_or(CarveError::Eof)?;

    *max_end = (*max_end).max(ifd_offset + total_len as u64);

    let mut strip_offsets: Option<Vec<u64>> = None;
    let mut strip_counts: Option<Vec<u64>> = None;
    let mut tile_offsets: Option<Vec<u64>> = None;
    let mut tile_counts: Option<Vec<u64>> = None;

    for i in 0..count as usize {
        let entry_start = 2 + i * 12;
        let entry = &ifd_buf[entry_start..entry_start + 12];
        let tag = read_u16(&entry[0..2], endian);
        let typ = read_u16(&entry[2..4], endian);
        let value_count = read_u32(&entry[4..8], endian) as u64;
        if value_count == 0 {
            continue;
        }
        let value_bytes = &entry[8..12];
        let type_size = match tiff_type_size(typ) {
            Some(size) => size as u64,
            None => continue,
        };
        let data_len = value_count.saturating_mul(type_size);

        if data_len > 4 {
            let data_offset = read_u32(value_bytes, endian) as u64;
            *max_end = (*max_end).max(data_offset.saturating_add(data_len));
        }

        if matches!(tag, TAG_SUB_IFD | TAG_EXIF_IFD | TAG_GPS_IFD) {
            let offsets =
                read_u32_array(ctx, start, endian, typ, value_count, value_bytes, data_len)?;
            for offset in offsets {
                if offset >= TIFF_HEADER_LEN as u64 {
                    queue.push_back(offset);
                }
            }
        }

        if tag == TAG_STRIP_OFFSETS {
            strip_offsets =
                Some(read_u32_array(ctx, start, endian, typ, value_count, value_bytes, data_len)?);
        } else if tag == TAG_STRIP_BYTE_COUNTS {
            strip_counts =
                Some(read_u32_array(ctx, start, endian, typ, value_count, value_bytes, data_len)?);
        } else if tag == TAG_TILE_OFFSETS {
            tile_offsets =
                Some(read_u32_array(ctx, start, endian, typ, value_count, value_bytes, data_len)?);
        } else if tag == TAG_TILE_BYTE_COUNTS {
            tile_counts =
                Some(read_u32_array(ctx, start, endian, typ, value_count, value_bytes, data_len)?);
        }
    }

    let next_ifd = read_u32(&ifd_buf[2 + entries_len..2 + entries_len + 4], endian);
    if next_ifd > 0 {
        queue.push_back(next_ifd as u64);
    }

    if let (Some(offsets), Some(counts)) = (strip_offsets, strip_counts) {
        update_max_with_offsets(offsets, counts, max_end);
    }
    if let (Some(offsets), Some(counts)) = (tile_offsets, tile_counts) {
        update_max_with_offsets(offsets, counts, max_end);
    }

    Ok(())
}

fn update_max_with_offsets(offsets: Vec<u64>, counts: Vec<u64>, max_end: &mut u64) {
    let len = std::cmp::min(offsets.len(), counts.len());
    for i in 0..len {
        let end = offsets[i].saturating_add(counts[i]);
        *max_end = (*max_end).max(end);
    }
}

fn read_u16(bytes: &[u8], endian: Endian) -> u16 {
    match endian {
        Endian::Little => u16::from_le_bytes([bytes[0], bytes[1]]),
        Endian::Big => u16::from_be_bytes([bytes[0], bytes[1]]),
    }
}

fn read_u32(bytes: &[u8], endian: Endian) -> u32 {
    match endian {
        Endian::Little => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        Endian::Big => u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
    }
}

fn tiff_type_size(typ: u16) -> Option<usize> {
    match typ {
        1 | 2 | 6 | 7 => Some(1),
        3 | 8 => Some(2),
        4 | 9 | 11 => Some(4),
        5 | 10 | 12 => Some(8),
        _ => None,
    }
}

fn read_u32_array(
    ctx: &ExtractionContext,
    start: u64,
    endian: Endian,
    typ: u16,
    count: u64,
    value_bytes: &[u8],
    data_len: u64,
) -> Result<Vec<u64>, CarveError> {
    if count > MAX_TIFF_ARRAY_ENTRIES {
        return Err(CarveError::Invalid("tiff array too large".to_string()));
    }
    let mut out = Vec::new();
    match typ {
        3 => {
            if data_len <= 4 {
                let mut i = 0u64;
                while i < count {
                    let idx = (i * 2) as usize;
                    if idx + 2 > value_bytes.len() {
                        break;
                    }
                    out.push(read_u16(&value_bytes[idx..idx + 2], endian) as u64);
                    i += 1;
                }
                return Ok(out);
            }
        }
        4 => {
            if data_len <= 4 {
                out.push(read_u32(value_bytes, endian) as u64);
                return Ok(out);
            }
        }
        _ => return Ok(out),
    }

    if data_len > MAX_TIFF_DATA_BYTES {
        return Err(CarveError::Invalid("tiff data too large".to_string()));
    }
    let data_offset = read_u32(value_bytes, endian) as u64;
    let abs = start.saturating_add(data_offset);
    let buf = read_exact_at(ctx, abs, data_len as usize).ok_or(CarveError::Eof)?;

    match typ {
        3 => {
            let mut i = 0usize;
            while i + 2 <= buf.len() && out.len() < count as usize {
                out.push(read_u16(&buf[i..i + 2], endian) as u64);
                i += 2;
            }
        }
        4 => {
            let mut i = 0usize;
            while i + 4 <= buf.len() && out.len() < count as usize {
                out.push(read_u32(&buf[i..i + 4], endian) as u64);
                i += 4;
            }
        }
        _ => {}
    }

    Ok(out)
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
    use super::TiffCarveHandler;
    use crate::carve::{CarveHandler, ExtractionContext};
    use crate::evidence::RawFileSource;
    use crate::scanner::NormalizedHit;

    #[test]
    fn carves_minimal_tiff() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_root = temp_dir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");

        let mut tiff = Vec::new();
        tiff.extend_from_slice(&[0x49, 0x49, 0x2A, 0x00]);
        tiff.extend_from_slice(&8u32.to_le_bytes());

        let ifd_offset = 8usize;
        let entry_count = 2u16;
        tiff.extend_from_slice(&entry_count.to_le_bytes());

        let strip_offset = (ifd_offset + 2 + 12 * 2 + 4) as u32;
        let strip_len = 4u32;

        tiff.extend_from_slice(&273u16.to_le_bytes());
        tiff.extend_from_slice(&4u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&strip_offset.to_le_bytes());

        tiff.extend_from_slice(&279u16.to_le_bytes());
        tiff.extend_from_slice(&4u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&strip_len.to_le_bytes());

        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.extend_from_slice(&[0u8; 4]);

        let input_path = temp_dir.path().join("image.bin");
        std::fs::write(&input_path, &tiff).expect("write tiff");

        let evidence = RawFileSource::open(&input_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "test",
            output_root: &output_root,
            evidence: &evidence,
        };
        let handler = TiffCarveHandler::new("tiff".to_string(), 8, 0);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "tiff".to_string(),
            pattern_id: "tiff_header".to_string(),
        };

        let carved = handler.process_hit(&hit, &ctx).expect("carve");
        let carved = carved.expect("carved");
        assert!(carved.validated);
        assert_eq!(carved.size, tiff.len() as u64);
    }
}
