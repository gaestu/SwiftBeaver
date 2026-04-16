use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crc32fast::Hasher as Crc32Hasher;
use flate2::read::DeflateDecoder;
use sha2::{Digest, Sha256};

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, DeferredWriter, ExtractionContext, PreValidation,
    output_path, write_range,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const ZIP_HEADER: &[u8] = b"PK\x03\x04";
const ZIP_EOCD: &[u8] = b"PK\x05\x06";
const ZIP_VERSION_MIN: u16 = 10;
const ZIP_VERSION_MAX: u16 = 63;
const ZIP_MAX_FILENAME_LEN: u16 = 1024;
const ZIP_EOCD_FIXED_LEN: usize = 22;
const ZIP_MAX_COMMENT_LEN: u64 = u16::MAX as u64;

pub struct ZipCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
    require_eocd: bool,
    allowed_kinds: Option<HashSet<String>>,
}

impl ZipCarveHandler {
    pub fn new(
        extension: String,
        min_size: u64,
        max_size: u64,
        require_eocd: bool,
        allowed_kinds: Option<Vec<String>>,
    ) -> Self {
        let allowed_kinds = allowed_kinds.map(|kinds| {
            kinds
                .into_iter()
                .map(|kind| kind.to_ascii_lowercase())
                .collect()
        });
        Self {
            extension,
            min_size,
            max_size,
            require_eocd,
            allowed_kinds,
        }
    }
}

impl CarveHandler for ZipCarveHandler {
    fn file_type(&self) -> &str {
        "zip"
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
        if buf != [0x50, 0x4B, 0x03, 0x04] {
            return Ok(PreValidation::Reject("zip signature mismatch".to_string()));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        let Some(_local_header) = read_local_header(ctx, hit.global_offset)? else {
            return Ok(None);
        };

        let mut validated = false;
        let mut truncated = false;
        let mut errors = Vec::new();
        let mut eocd: Option<ZipEocd> = None;
        let mut bytes_written = 0u64;

        let (full_path, mut rel_path) = if self.require_eocd {
            let Some((eocd_offset, parsed)) = find_eocd(ctx, hit.global_offset, self.max_size)?
            else {
                return Ok(None);
            };
            let comment_len = parsed.comment_len;
            eocd = Some(parsed);

            let mut total_end = eocd_offset + 22 + comment_len as u64;
            if self.max_size > 0 {
                let max_end = hit.global_offset + self.max_size;
                if total_end > max_end {
                    total_end = max_end;
                    truncated = true;
                    errors.push("max_size reached after EOCD".to_string());
                }
            }

            let (mut full_path, mut rel_path) = output_path(
                ctx.output_root,
                self.file_type(),
                &self.extension,
                hit.global_offset,
            )?;
            let mut md5 = md5::Context::new();
            let mut sha256 = Sha256::new();

            let (written, eof_truncated) = write_range(
                ctx,
                hit.global_offset,
                total_end,
                &full_path,
                &mut md5,
                &mut sha256,
            )?;
            bytes_written = written;
            if eof_truncated {
                truncated = true;
                errors.push("eof before EOCD end".to_string());
            }

            if bytes_written < self.min_size {
                let _ = std::fs::remove_file(&full_path);
                return Ok(None);
            }

            match validate_zip_archive(&full_path) {
                Ok(()) => {
                    validated = true;
                }
                Err(err) => {
                    errors.push(format!("zip archive validation failed: {err}"));
                }
            }

            let md5_hex = format!("{:x}", md5.compute());
            let sha256_hex = hex::encode(sha256.finalize());
            let global_end = if bytes_written == 0 {
                hit.global_offset
            } else {
                hit.global_offset + bytes_written - 1
            };

            let mut file_type = self.file_type().to_string();
            let mut extension = self.extension.clone();

            if let Some(parsed) = &eocd
                && let Some(kind) = classify_zip(&full_path, parsed.cd_offset, parsed.cd_size)
            {
                file_type = kind.file_type().to_string();
                extension = kind.extension().to_string();
                if file_type != self.file_type()
                    && let Ok((new_path, new_rel)) =
                        output_path(ctx.output_root, &file_type, &extension, hit.global_offset)
                    && std::fs::rename(&full_path, &new_path).is_ok()
                {
                    rel_path = new_rel;
                    full_path = new_path;
                }
            }

            if let Some(allowed) = &self.allowed_kinds
                && !allowed.contains(&file_type)
            {
                let _ = std::fs::remove_file(&full_path);
                return Ok(None);
            }

            return Ok(Some(CarvedFile {
                run_id: ctx.run_id.to_string(),
                file_type,
                path: rel_path,
                extension,
                global_start: hit.global_offset,
                global_end,
                size: bytes_written,
                md5: Some(md5_hex),
                sha256: Some(sha256_hex),
                validated,
                truncated,
                errors,
                pattern_id: Some(hit.pattern_id.clone()),
            }));
        } else {
            output_path(
                ctx.output_root,
                self.file_type(),
                &self.extension,
                hit.global_offset,
            )?
        };

        let mut writer = DeferredWriter::new(full_path.clone(), ctx.deferred_buffer_bytes);
        let mut md5 = md5::Context::new();
        let mut sha256 = Sha256::new();

        let mut offset = hit.global_offset;
        let mut carry: Vec<u8> = Vec::new();
        let buf_size = 64 * 1024;

        loop {
            if self.max_size > 0 && bytes_written >= self.max_size {
                truncated = true;
                errors.push("max_size reached before EOCD".to_string());
                break;
            }

            let remaining = if self.max_size > 0 {
                (self.max_size - bytes_written).min(buf_size as u64)
            } else {
                buf_size as u64
            };

            let mut buf = vec![0u8; remaining as usize];
            let n = ctx
                .evidence
                .read_at(offset, &mut buf)
                .map_err(|e| CarveError::Evidence(e.to_string()))?;
            if n == 0 {
                truncated = true;
                errors.push("eof before EOCD".to_string());
                break;
            }
            buf.truncate(n);

            if bytes_written == 0
                && buf.len() >= ZIP_HEADER.len()
                && &buf[..ZIP_HEADER.len()] != ZIP_HEADER
            {
                writer.discard();
                return Ok(None);
            }

            let mut search_buf = carry.clone();
            search_buf.extend_from_slice(&buf);
            if let Some(pos) = find_pattern(&search_buf, ZIP_EOCD) {
                let eocd_offset = offset.saturating_sub(carry.len() as u64) + pos as u64;
                if let Ok(parsed) = read_eocd(ctx, eocd_offset) {
                    eocd = Some(parsed);
                }

                let mut total_end = if let Some(parsed) = &eocd {
                    eocd_offset + 22 + parsed.comment_len as u64
                } else {
                    eocd_offset + 22
                };

                if self.max_size > 0 {
                    let max_end = hit.global_offset + self.max_size;
                    if total_end > max_end {
                        total_end = max_end;
                        truncated = true;
                        errors.push("max_size reached after EOCD".to_string());
                    }
                }

                let write_len = if total_end <= offset {
                    0usize
                } else {
                    let remaining = (total_end - offset) as usize;
                    remaining.min(buf.len())
                };

                if write_len > 0 {
                    let slice = &buf[..write_len];
                    writer.write_all(slice)?;
                    md5.consume(slice);
                    sha256.update(slice);
                    bytes_written = bytes_written.saturating_add(slice.len() as u64);
                }

                if total_end > offset + write_len as u64 {
                    let mut extra_offset = offset + write_len as u64;
                    let mut remaining = total_end - extra_offset;
                    while remaining > 0 {
                        let read_len = remaining.min(buf_size as u64) as usize;
                        let mut extra = vec![0u8; read_len];
                        let n = ctx
                            .evidence
                            .read_at(extra_offset, &mut extra)
                            .map_err(|e| CarveError::Evidence(e.to_string()))?;
                        if n == 0 {
                            truncated = true;
                            errors.push("eof before EOCD end".to_string());
                            break;
                        }
                        extra.truncate(n);
                        writer.write_all(&extra)?;
                        md5.consume(&extra);
                        sha256.update(&extra);
                        bytes_written = bytes_written.saturating_add(extra.len() as u64);
                        extra_offset = extra_offset.saturating_add(extra.len() as u64);
                        remaining = remaining.saturating_sub(extra.len() as u64);
                    }
                }

                break;
            }

            writer.write_all(&buf)?;
            md5.consume(&buf);
            sha256.update(&buf);
            bytes_written = bytes_written.saturating_add(buf.len() as u64);
            offset = offset.saturating_add(buf.len() as u64);

            carry = if buf.len() >= ZIP_EOCD.len() - 1 {
                buf[buf.len() - (ZIP_EOCD.len() - 1)..].to_vec()
            } else {
                buf.clone()
            };
        }

        writer.flush_to_disk()?;

        if bytes_written < self.min_size {
            let _ = std::fs::remove_file(&full_path);
            return Ok(None);
        }

        match validate_zip_archive(&full_path) {
            Ok(()) => {
                validated = true;
            }
            Err(err) => {
                errors.push(format!("zip archive validation failed: {err}"));
            }
        }

        let md5_hex = format!("{:x}", md5.compute());
        let sha256_hex = hex::encode(sha256.finalize());
        let global_end = if bytes_written == 0 {
            hit.global_offset
        } else {
            hit.global_offset + bytes_written - 1
        };

        let mut file_type = self.file_type().to_string();
        let mut extension = self.extension.clone();

        if let Some(parsed) = &eocd
            && let Some(kind) = classify_zip(&full_path, parsed.cd_offset, parsed.cd_size)
        {
            file_type = kind.file_type().to_string();
            extension = kind.extension().to_string();
            if file_type != self.file_type()
                && let Ok((new_path, new_rel)) =
                    output_path(ctx.output_root, &file_type, &extension, hit.global_offset)
                && std::fs::rename(&full_path, &new_path).is_ok()
            {
                rel_path = new_rel;
            }
        }

        Ok(Some(CarvedFile {
            run_id: ctx.run_id.to_string(),
            file_type,
            path: rel_path,
            extension,
            global_start: hit.global_offset,
            global_end,
            size: bytes_written,
            md5: Some(md5_hex),
            sha256: Some(sha256_hex),
            validated,
            truncated,
            errors,
            pattern_id: Some(hit.pattern_id.clone()),
        }))
    }
}

struct ZipLocalHeader {
    _flags: u16,
    _compressed_size: u64,
    _data_offset: u64,
}

fn read_local_header(
    ctx: &ExtractionContext,
    offset: u64,
) -> Result<Option<ZipLocalHeader>, CarveError> {
    let mut buf = [0u8; 30];
    let n = ctx
        .evidence
        .read_at(offset, &mut buf)
        .map_err(|e| CarveError::Evidence(e.to_string()))?;
    if n < buf.len() {
        return Ok(None);
    }
    if &buf[0..4] != ZIP_HEADER {
        return Ok(None);
    }

    let version_needed = u16::from_le_bytes([buf[4], buf[5]]);
    if !(ZIP_VERSION_MIN..=ZIP_VERSION_MAX).contains(&version_needed) {
        return Ok(None);
    }

    let flags = u16::from_le_bytes([buf[6], buf[7]]);
    if flags & 0xC000 != 0 {
        return Ok(None);
    }

    let method = u16::from_le_bytes([buf[8], buf[9]]);
    if !is_supported_zip_method(method) {
        return Ok(None);
    }

    let compressed_size = u32::from_le_bytes([buf[18], buf[19], buf[20], buf[21]]) as u64;
    let uncompressed_size = u32::from_le_bytes([buf[22], buf[23], buf[24], buf[25]]) as u64;
    let file_name_len = u16::from_le_bytes([buf[26], buf[27]]);
    let extra_len = u16::from_le_bytes([buf[28], buf[29]]);

    if file_name_len == 0 || file_name_len > ZIP_MAX_FILENAME_LEN {
        return Ok(None);
    }

    let data_offset = match offset
        .checked_add(30)
        .and_then(|v| v.checked_add(file_name_len as u64))
        .and_then(|v| v.checked_add(extra_len as u64))
    {
        Some(v) => v,
        None => return Ok(None),
    };
    if data_offset >= ctx.evidence.len() {
        return Ok(None);
    }

    // Bit 3 indicates data descriptor: sizes may be absent/zero in local header.
    let has_data_descriptor = flags & (1 << 3) != 0;
    if !has_data_descriptor {
        if compressed_size == 0 && uncompressed_size > 0 {
            return Ok(None);
        }
        if compressed_size != 0xFFFF_FFFF {
            let available = ctx.evidence.len().saturating_sub(data_offset);
            if compressed_size > available {
                return Ok(None);
            }
        }
    }

    Ok(Some(ZipLocalHeader {
        _flags: flags,
        _compressed_size: compressed_size,
        _data_offset: data_offset,
    }))
}

fn is_supported_zip_method(method: u16) -> bool {
    matches!(
        method,
        0 | 1
            | 2
            | 3
            | 4
            | 5
            | 6
            | 7
            | 8
            | 9
            | 10
            | 12
            | 14
            | 18
            | 19
            | 20
            | 93
            | 94
            | 95
            | 96
            | 97
            | 98
            | 99
    )
}

fn find_eocd(
    ctx: &ExtractionContext,
    start: u64,
    max_size: u64,
) -> Result<Option<(u64, ZipEocd)>, CarveError> {
    let mut offset = start;
    let mut bytes_scanned = 0u64;
    let mut carry: Vec<u8> = Vec::new();
    let buf_size = 64 * 1024;
    let mut last_valid: Option<(u64, ZipEocd)> = None;

    loop {
        if max_size > 0 && bytes_scanned >= max_size {
            return Ok(last_valid);
        }

        let remaining = if max_size > 0 {
            (max_size - bytes_scanned).min(buf_size as u64)
        } else {
            buf_size as u64
        };

        let mut buf = vec![0u8; remaining as usize];
        let n = ctx
            .evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n == 0 {
            return Ok(last_valid);
        }
        buf.truncate(n);

        if bytes_scanned == 0
            && buf.len() >= ZIP_HEADER.len()
            && &buf[..ZIP_HEADER.len()] != ZIP_HEADER
        {
            return Ok(None);
        }

        let mut search_buf = carry.clone();
        search_buf.extend_from_slice(&buf);
        let mut search_start = 0usize;
        while let Some(pos) = find_pattern(&search_buf[search_start..], ZIP_EOCD) {
            let absolute = search_start + pos;
            let eocd_offset = offset.saturating_sub(carry.len() as u64) + absolute as u64;
            if let Ok(parsed) = read_eocd(ctx, eocd_offset) {
                let expected = start
                    .saturating_add(parsed.cd_offset)
                    .saturating_add(parsed.cd_size);
                if expected == eocd_offset {
                    last_valid = Some((eocd_offset, parsed));
                }
            }
            search_start = absolute + 1;
        }

        bytes_scanned = bytes_scanned.saturating_add(buf.len() as u64);
        offset = offset.saturating_add(buf.len() as u64);
        carry = if buf.len() >= ZIP_EOCD.len() - 1 {
            buf[buf.len() - (ZIP_EOCD.len() - 1)..].to_vec()
        } else {
            buf.clone()
        };
    }
}

#[derive(Debug, Clone)]
struct ZipEocd {
    cd_offset: u64,
    cd_size: u64,
    total_entries: u16,
    comment_len: u16,
}

fn read_eocd(ctx: &ExtractionContext, offset: u64) -> Result<ZipEocd, CarveError> {
    let mut buf = [0u8; 22];
    let n = ctx
        .evidence
        .read_at(offset, &mut buf)
        .map_err(|e| CarveError::Evidence(e.to_string()))?;
    if n < 22 {
        return Err(CarveError::Eof);
    }
    parse_eocd_bytes(&buf)
}

fn parse_eocd_bytes(buf: &[u8]) -> Result<ZipEocd, CarveError> {
    if buf.len() < ZIP_EOCD_FIXED_LEN {
        return Err(CarveError::Eof);
    }
    if &buf[0..4] != ZIP_EOCD {
        return Err(CarveError::Invalid(
            "zip eocd signature mismatch".to_string(),
        ));
    }
    let total_entries = u16::from_le_bytes([buf[10], buf[11]]);
    let cd_size = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]) as u64;
    let cd_offset = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]) as u64;
    let comment_len = u16::from_le_bytes([buf[20], buf[21]]);
    Ok(ZipEocd {
        cd_offset,
        cd_size,
        total_entries,
        comment_len,
    })
}

#[derive(Debug, Clone)]
struct ZipArchiveEntry {
    local_header_offset: u64,
    compression_method: u16,
    flags: u16,
    crc32: u32,
    compressed_size: u64,
    uncompressed_size: u64,
}

fn validate_zip_archive(path: &Path) -> Result<(), CarveError> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let (eocd_offset, eocd) = find_archive_eocd(&mut file, file_len)?;
    let entries = parse_archive_central_directory(&mut file, file_len, eocd_offset, &eocd)?;
    for entry in &entries {
        validate_archive_entry(&mut file, file_len, entry)?;
    }
    Ok(())
}

fn find_archive_eocd(file: &mut File, file_len: u64) -> Result<(u64, ZipEocd), CarveError> {
    if file_len < ZIP_EOCD_FIXED_LEN as u64 {
        return Err(CarveError::Invalid(
            "zip file too small for EOCD".to_string(),
        ));
    }

    let scan_len = file_len.min(ZIP_EOCD_FIXED_LEN as u64 + ZIP_MAX_COMMENT_LEN);
    let scan_start = file_len.saturating_sub(scan_len);
    file.seek(SeekFrom::Start(scan_start))?;
    let mut tail = vec![0u8; scan_len as usize];
    file.read_exact(&mut tail)?;

    if tail.len() < 4 {
        return Err(CarveError::Invalid("zip EOCD not found".to_string()));
    }

    for idx in (0..=tail.len() - 4).rev() {
        if &tail[idx..idx + 4] != ZIP_EOCD {
            continue;
        }
        if idx + ZIP_EOCD_FIXED_LEN > tail.len() {
            continue;
        }
        let eocd = parse_eocd_bytes(&tail[idx..idx + ZIP_EOCD_FIXED_LEN])?;
        let expected_end = idx + ZIP_EOCD_FIXED_LEN + eocd.comment_len as usize;
        if expected_end != tail.len() {
            continue;
        }
        let eocd_offset = scan_start + idx as u64;
        return Ok((eocd_offset, eocd));
    }

    Err(CarveError::Invalid(
        "zip EOCD not found at archive end".to_string(),
    ))
}

fn parse_archive_central_directory(
    file: &mut File,
    file_len: u64,
    eocd_offset: u64,
    eocd: &ZipEocd,
) -> Result<Vec<ZipArchiveEntry>, CarveError> {
    if eocd.total_entries == u16::MAX {
        return Err(CarveError::Invalid(
            "zip64 archives are not supported for strict validation".to_string(),
        ));
    }
    if eocd.cd_size == u32::MAX as u64 || eocd.cd_offset == u32::MAX as u64 {
        return Err(CarveError::Invalid(
            "zip64 central directory is not supported for strict validation".to_string(),
        ));
    }

    let cd_end = eocd
        .cd_offset
        .checked_add(eocd.cd_size)
        .ok_or_else(|| CarveError::Invalid("zip central directory overflow".to_string()))?;
    if cd_end > eocd_offset || cd_end > file_len {
        return Err(CarveError::Invalid(
            "zip central directory exceeds archive bounds".to_string(),
        ));
    }

    file.seek(SeekFrom::Start(eocd.cd_offset))?;
    let mut cd = vec![0u8; eocd.cd_size as usize];
    if !cd.is_empty() {
        file.read_exact(&mut cd)?;
    }

    let mut entries = Vec::new();
    let mut idx = 0usize;
    while idx < cd.len() {
        if idx + 46 > cd.len() {
            return Err(CarveError::Invalid(
                "zip central directory entry truncated".to_string(),
            ));
        }
        if &cd[idx..idx + 4] != b"PK\x01\x02" {
            return Err(CarveError::Invalid(
                "zip central directory signature mismatch".to_string(),
            ));
        }

        let flags = u16::from_le_bytes([cd[idx + 8], cd[idx + 9]]);
        let method = u16::from_le_bytes([cd[idx + 10], cd[idx + 11]]);
        let crc32 = u32::from_le_bytes([cd[idx + 16], cd[idx + 17], cd[idx + 18], cd[idx + 19]]);
        let compressed_size =
            u32::from_le_bytes([cd[idx + 20], cd[idx + 21], cd[idx + 22], cd[idx + 23]]) as u64;
        let uncompressed_size =
            u32::from_le_bytes([cd[idx + 24], cd[idx + 25], cd[idx + 26], cd[idx + 27]]) as u64;
        let name_len = u16::from_le_bytes([cd[idx + 28], cd[idx + 29]]) as usize;
        let extra_len = u16::from_le_bytes([cd[idx + 30], cd[idx + 31]]) as usize;
        let comment_len = u16::from_le_bytes([cd[idx + 32], cd[idx + 33]]) as usize;
        let local_header_offset =
            u32::from_le_bytes([cd[idx + 42], cd[idx + 43], cd[idx + 44], cd[idx + 45]]) as u64;

        let next = idx
            .checked_add(46)
            .and_then(|v| v.checked_add(name_len))
            .and_then(|v| v.checked_add(extra_len))
            .and_then(|v| v.checked_add(comment_len))
            .ok_or_else(|| CarveError::Invalid("zip central directory overflow".to_string()))?;
        if next > cd.len() {
            return Err(CarveError::Invalid(
                "zip central directory entry exceeds bounds".to_string(),
            ));
        }

        entries.push(ZipArchiveEntry {
            local_header_offset,
            compression_method: method,
            flags,
            crc32,
            compressed_size,
            uncompressed_size,
        });

        idx = next;
    }

    if entries.len() != eocd.total_entries as usize {
        return Err(CarveError::Invalid(format!(
            "zip central directory entry count mismatch: expected {}, got {}",
            eocd.total_entries,
            entries.len()
        )));
    }

    Ok(entries)
}

fn validate_archive_entry(
    file: &mut File,
    file_len: u64,
    entry: &ZipArchiveEntry,
) -> Result<(), CarveError> {
    if entry.local_header_offset >= file_len {
        return Err(CarveError::Invalid(
            "zip local header offset outside archive".to_string(),
        ));
    }

    file.seek(SeekFrom::Start(entry.local_header_offset))?;
    let mut header = [0u8; 30];
    file.read_exact(&mut header)?;
    if &header[0..4] != ZIP_HEADER {
        return Err(CarveError::Invalid(
            "zip local header signature mismatch".to_string(),
        ));
    }

    let local_flags = u16::from_le_bytes([header[6], header[7]]);
    let local_method = u16::from_le_bytes([header[8], header[9]]);
    if local_method != entry.compression_method {
        return Err(CarveError::Invalid(
            "zip local/central compression method mismatch".to_string(),
        ));
    }

    let name_len = u16::from_le_bytes([header[26], header[27]]) as u64;
    let extra_len = u16::from_le_bytes([header[28], header[29]]) as u64;
    let data_offset = entry
        .local_header_offset
        .checked_add(30)
        .and_then(|v| v.checked_add(name_len))
        .and_then(|v| v.checked_add(extra_len))
        .ok_or_else(|| CarveError::Invalid("zip local header overflow".to_string()))?;
    let data_end = data_offset
        .checked_add(entry.compressed_size)
        .ok_or_else(|| CarveError::Invalid("zip entry data overflow".to_string()))?;
    if data_end > file_len {
        return Err(CarveError::Invalid(
            "zip entry data exceeds archive bounds".to_string(),
        ));
    }

    // Bit 3 indicates data descriptor; sizes/CRC in local header may be zero, but
    // central directory values are authoritative.
    let _has_data_descriptor = (entry.flags | local_flags) & (1 << 3) != 0;

    file.seek(SeekFrom::Start(data_offset))?;
    let (crc32, uncompressed_size) = match entry.compression_method {
        0 => {
            let mut reader = (&mut *file).take(entry.compressed_size);
            let (crc, size) = crc32_and_size(&mut reader)?;
            if size != entry.compressed_size {
                return Err(CarveError::Invalid(
                    "stored zip entry size mismatch".to_string(),
                ));
            }
            (crc, size)
        }
        8 => {
            let reader = (&mut *file).take(entry.compressed_size);
            let mut decoder = DeflateDecoder::new(reader);
            let (crc, size) = crc32_and_size(&mut decoder)?;
            if decoder.get_ref().limit() != 0 {
                return Err(CarveError::Invalid(
                    "deflate zip entry has trailing compressed bytes".to_string(),
                ));
            }
            (crc, size)
        }
        method => {
            return Err(CarveError::Invalid(format!(
                "unsupported zip compression method for strict validation: {}",
                method
            )));
        }
    };

    if crc32 != entry.crc32 {
        return Err(CarveError::Invalid("zip entry CRC32 mismatch".to_string()));
    }
    if uncompressed_size != entry.uncompressed_size {
        return Err(CarveError::Invalid(
            "zip entry uncompressed size mismatch".to_string(),
        ));
    }
    Ok(())
}

fn crc32_and_size<R: Read>(reader: &mut R) -> Result<(u32, u64), CarveError> {
    let mut crc = Crc32Hasher::new();
    let mut total = 0u64;
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        crc.update(&buf[..n]);
        total = total.saturating_add(n as u64);
    }
    Ok((crc.finalize(), total))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ZipKind {
    Docx,
    Xlsx,
    Pptx,
    Odt,
    Ods,
    Odp,
    Epub,
}

impl ZipKind {
    fn file_type(self) -> &'static str {
        match self {
            ZipKind::Docx => "docx",
            ZipKind::Xlsx => "xlsx",
            ZipKind::Pptx => "pptx",
            ZipKind::Odt => "odt",
            ZipKind::Ods => "ods",
            ZipKind::Odp => "odp",
            ZipKind::Epub => "epub",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            ZipKind::Docx => "docx",
            ZipKind::Xlsx => "xlsx",
            ZipKind::Pptx => "pptx",
            ZipKind::Odt => "odt",
            ZipKind::Ods => "ods",
            ZipKind::Odp => "odp",
            ZipKind::Epub => "epub",
        }
    }
}

struct ZipEntryInfo {
    local_header_offset: u64,
    compressed_size: u64,
    compression_method: u16,
}

fn classify_zip(path: &Path, cd_offset: u64, cd_size: u64) -> Option<ZipKind> {
    if cd_size == 0 || cd_size > 16 * 1024 * 1024 {
        return None;
    }

    let mut file = File::open(path).ok()?;
    if file.seek(SeekFrom::Start(cd_offset)).is_err() {
        return None;
    }

    let mut buf = vec![0u8; cd_size as usize];
    if file.read_exact(&mut buf).is_err() {
        return None;
    }

    let mut mimetype_entry: Option<ZipEntryInfo> = None;
    let mut idx = 0usize;
    while idx + 46 <= buf.len() {
        if &buf[idx..idx + 4] != b"PK\x01\x02" {
            break;
        }
        let compression = u16::from_le_bytes([buf[idx + 10], buf[idx + 11]]);
        let comp_size =
            u32::from_le_bytes([buf[idx + 20], buf[idx + 21], buf[idx + 22], buf[idx + 23]]) as u64;
        let name_len = u16::from_le_bytes([buf[idx + 28], buf[idx + 29]]) as usize;
        let extra_len = u16::from_le_bytes([buf[idx + 30], buf[idx + 31]]) as usize;
        let comment_len = u16::from_le_bytes([buf[idx + 32], buf[idx + 33]]) as usize;
        let local_header_offset =
            u32::from_le_bytes([buf[idx + 42], buf[idx + 43], buf[idx + 44], buf[idx + 45]]) as u64;
        let name_start = idx + 46;
        let name_end = name_start + name_len;
        if name_end > buf.len() {
            break;
        }
        let name = &buf[name_start..name_end];
        if name.starts_with(b"word/") {
            return Some(ZipKind::Docx);
        }
        if name.starts_with(b"xl/") {
            return Some(ZipKind::Xlsx);
        }
        if name.starts_with(b"ppt/") {
            return Some(ZipKind::Pptx);
        }
        if name == b"mimetype" {
            mimetype_entry = Some(ZipEntryInfo {
                local_header_offset,
                compressed_size: comp_size,
                compression_method: compression,
            });
        }
        idx = name_end + extra_len + comment_len;
    }

    if let Some(entry) = mimetype_entry
        && let Some(mime) = read_stored_entry(path, &entry)
    {
        let mime = trim_ascii(&mime);
        if mime == b"application/vnd.oasis.opendocument.text" {
            return Some(ZipKind::Odt);
        }
        if mime == b"application/vnd.oasis.opendocument.spreadsheet" {
            return Some(ZipKind::Ods);
        }
        if mime == b"application/vnd.oasis.opendocument.presentation" {
            return Some(ZipKind::Odp);
        }
        if mime == b"application/epub+zip" {
            return Some(ZipKind::Epub);
        }
    }

    None
}

fn read_stored_entry(path: &Path, entry: &ZipEntryInfo) -> Option<Vec<u8>> {
    if entry.compression_method != 0 || entry.compressed_size > 1024 {
        return None;
    }
    let mut file = File::open(path).ok()?;
    if file
        .seek(SeekFrom::Start(entry.local_header_offset))
        .is_err()
    {
        return None;
    }
    let mut header = [0u8; 30];
    if file.read_exact(&mut header).is_err() {
        return None;
    }
    if &header[0..4] != b"PK\x03\x04" {
        return None;
    }
    let name_len = u16::from_le_bytes([header[26], header[27]]) as u64;
    let extra_len = u16::from_le_bytes([header[28], header[29]]) as u64;
    let data_offset = entry
        .local_header_offset
        .saturating_add(30)
        .saturating_add(name_len)
        .saturating_add(extra_len);
    if file.seek(SeekFrom::Start(data_offset)).is_err() {
        return None;
    }
    let mut data = vec![0u8; entry.compressed_size as usize];
    if file.read_exact(&mut data).is_err() {
        return None;
    }
    Some(data)
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let mut start = 0usize;
    let mut end = bytes.len();
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    &bytes[start..end]
}

fn find_pattern(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let first = needle[0];
    let mut i = 0usize;
    while i + needle.len() <= haystack.len() {
        if haystack[i] == first && &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{ZipCarveHandler, ZipKind, classify_zip};
    use crate::carve::{CarveHandler, ExtractionContext};
    use crate::evidence::RawFileSource;
    use crate::scanner::NormalizedHit;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn classifies_docx_by_entries() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("test.zip");
        let mut file = File::create(&path).expect("create");
        let data = sample_zip_with_entry("word/document.xml");
        file.write_all(&data).expect("write");
        drop(file);

        let kind = classify_zip(&path, 48, 63);
        assert_eq!(kind, Some(ZipKind::Docx));
    }

    #[test]
    fn classifies_odt_by_mimetype() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("odt.zip");
        let (data, cd_offset, cd_size) =
            sample_zip_with_mimetype("application/vnd.oasis.opendocument.text");
        let mut file = File::create(&path).expect("create");
        file.write_all(&data).expect("write");
        drop(file);

        let kind = classify_zip(&path, cd_offset, cd_size);
        assert_eq!(kind, Some(ZipKind::Odt));
    }

    fn sample_zip_with_entry(name: &str) -> Vec<u8> {
        sample_zip_with_entry_payload(name, b"x", 0)
    }

    fn sample_valid_stored_zip(name: &str, payload: &[u8]) -> Vec<u8> {
        let crc = crc32fast::hash(payload);
        sample_zip_with_entry_payload(name, payload, crc)
    }

    fn sample_zip_with_entry_payload(name: &str, payload: &[u8], crc32: u32) -> Vec<u8> {
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len() as u16;
        let payload_len = payload.len() as u32;
        let mut out = Vec::new();

        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&[0x14, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&crc32.to_le_bytes());
        out.extend_from_slice(&payload_len.to_le_bytes());
        out.extend_from_slice(&payload_len.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(payload);

        out.extend_from_slice(b"PK\x01\x02");
        out.extend_from_slice(&[0x14, 0x00]);
        out.extend_from_slice(&[0x14, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&crc32.to_le_bytes());
        out.extend_from_slice(&payload_len.to_le_bytes());
        out.extend_from_slice(&payload_len.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        out.extend_from_slice(name_bytes);

        let cd_size = 46 + name_bytes.len();
        let cd_offset = 30 + name_bytes.len() + payload.len();

        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x01, 0x00]);
        out.extend_from_slice(&[0x01, 0x00]);
        out.extend_from_slice(&(cd_size as u32).to_le_bytes());
        out.extend_from_slice(&(cd_offset as u32).to_le_bytes());
        out.extend_from_slice(&[0x00, 0x00]);

        out
    }

    fn sample_zip_with_mimetype(mime: &str) -> (Vec<u8>, u64, u64) {
        let name_bytes = b"mimetype";
        let name_len = name_bytes.len() as u16;
        let data_bytes = mime.as_bytes();
        let data_len = data_bytes.len() as u32;
        let mut out = Vec::new();

        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&[0x14, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        out.extend_from_slice(&data_len.to_le_bytes());
        out.extend_from_slice(&data_len.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(data_bytes);

        let local_header_len = 30 + name_bytes.len() + data_bytes.len();

        out.extend_from_slice(b"PK\x01\x02");
        out.extend_from_slice(&[0x14, 0x00]);
        out.extend_from_slice(&[0x14, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        out.extend_from_slice(&data_len.to_le_bytes());
        out.extend_from_slice(&data_len.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        out.extend_from_slice(name_bytes);

        let cd_size = 46 + name_bytes.len();
        let cd_offset = local_header_len;

        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x01, 0x00]);
        out.extend_from_slice(&[0x01, 0x00]);
        out.extend_from_slice(&(cd_size as u32).to_le_bytes());
        out.extend_from_slice(&(cd_offset as u32).to_le_bytes());
        out.extend_from_slice(&[0x00, 0x00]);

        (out, cd_offset as u64, cd_size as u64)
    }

    #[test]
    fn rejects_zip_without_eocd_when_required() {
        let dir = tempdir().expect("tempdir");
        let evidence_path = dir.path().join("evidence.bin");
        let mut file = File::create(&evidence_path).expect("create");
        file.write_all(b"PK\x03\x04\x00\x00\x00\x00\x00\x00")
            .expect("write");
        drop(file);

        let evidence = RawFileSource::open(&evidence_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "run",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
        };
        let handler = ZipCarveHandler::new("zip".to_string(), 0, 1024, true, None);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "zip".to_string(),
            pattern_id: "zip_header".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        assert!(result.is_none());
        assert!(!dir.path().join("zip").exists());
    }

    #[test]
    fn filters_zip_kinds_when_configured() {
        let dir = tempdir().expect("tempdir");
        let evidence_path = dir.path().join("evidence.bin");
        let mut file = File::create(&evidence_path).expect("create");
        let data = sample_zip_with_entry("word/document.xml");
        file.write_all(&data).expect("write");
        drop(file);

        let evidence = RawFileSource::open(&evidence_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "run",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
        };
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "zip".to_string(),
            pattern_id: "zip_header".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };

        let handler = ZipCarveHandler::new(
            "zip".to_string(),
            0,
            1024,
            true,
            Some(vec!["docx".to_string()]),
        );
        let result = handler.process_hit(&hit, &ctx).expect("process");
        let carved = result.expect("carved");
        assert_eq!(carved.file_type, "docx");
        assert!(dir.path().join("docx").exists());

        let dir = tempdir().expect("tempdir");
        let evidence_path = dir.path().join("evidence.bin");
        let mut file = File::create(&evidence_path).expect("create");
        file.write_all(&data).expect("write");
        drop(file);

        let evidence = RawFileSource::open(&evidence_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "run",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
        };
        let handler = ZipCarveHandler::new(
            "zip".to_string(),
            0,
            1024,
            true,
            Some(vec!["xlsx".to_string()]),
        );
        let result = handler.process_hit(&hit, &ctx).expect("process");
        assert!(result.is_none());
        assert!(!dir.path().join("xlsx").exists());
    }

    #[test]
    fn rejects_invalid_local_header_fields() {
        let dir = tempdir().expect("tempdir");
        let evidence_path = dir.path().join("evidence.bin");
        let mut file = File::create(&evidence_path).expect("create");

        let mut data = Vec::new();
        data.extend_from_slice(b"PK\x03\x04");
        data.extend_from_slice(&0u16.to_le_bytes()); // invalid version
        data.extend_from_slice(&0u16.to_le_bytes()); // flags
        data.extend_from_slice(&8u16.to_le_bytes()); // method
        data.extend_from_slice(&0u16.to_le_bytes()); // mod time
        data.extend_from_slice(&0u16.to_le_bytes()); // mod date
        data.extend_from_slice(&0u32.to_le_bytes()); // crc32
        data.extend_from_slice(&1u32.to_le_bytes()); // comp size
        data.extend_from_slice(&1u32.to_le_bytes()); // uncomp size
        data.extend_from_slice(&5u16.to_le_bytes()); // file name len
        data.extend_from_slice(&0u16.to_le_bytes()); // extra len
        data.extend_from_slice(b"a.txt");
        data.push(0x41);
        file.write_all(&data).expect("write");
        drop(file);

        let evidence = RawFileSource::open(&evidence_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "run",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
        };
        let handler = ZipCarveHandler::new("zip".to_string(), 0, 1024, true, None);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "zip".to_string(),
            pattern_id: "zip_header".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        assert!(result.is_none());
    }

    #[test]
    fn marks_crc_mismatch_as_unvalidated() {
        let dir = tempdir().expect("tempdir");
        let evidence_path = dir.path().join("evidence.bin");
        let mut file = File::create(&evidence_path).expect("create");

        // Uses CRC32=0 for payload "x", so strict archive validation must fail.
        let data = sample_zip_with_entry("word/document.xml");
        file.write_all(&data).expect("write");
        drop(file);

        let evidence = RawFileSource::open(&evidence_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "run",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
        };
        let handler = ZipCarveHandler::new("zip".to_string(), 0, 1024, true, None);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "zip".to_string(),
            pattern_id: "zip_header".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        let carved = result.expect("carved");
        assert!(!carved.validated);
        assert!(
            carved
                .errors
                .iter()
                .any(|e| e.contains("zip archive validation failed"))
        );
    }

    #[test]
    fn validates_well_formed_stored_zip() {
        let dir = tempdir().expect("tempdir");
        let evidence_path = dir.path().join("evidence.bin");
        let mut file = File::create(&evidence_path).expect("create");
        let data = sample_valid_stored_zip("hello.txt", b"hello zip");
        file.write_all(&data).expect("write");
        drop(file);

        let evidence = RawFileSource::open(&evidence_path).expect("evidence");
        let ctx = ExtractionContext {
            run_id: "run",
            output_root: dir.path(),
            evidence: &evidence,
            deferred_buffer_bytes: 0,
            io_buf: std::cell::RefCell::new(Vec::new()),
            chunk_data: None,
            chunk_start: 0,
        };
        let handler = ZipCarveHandler::new("zip".to_string(), 0, 2048, true, None);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "zip".to_string(),
            pattern_id: "zip_header".to_string(),
            chunk_data: None,
            chunk_start: 0,
        };

        let result = handler.process_hit(&hit, &ctx).expect("process");
        let carved = result.expect("carved");
        assert!(carved.validated);
        assert!(carved.errors.is_empty());
    }
}
