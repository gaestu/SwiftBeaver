use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::carve::{
    output_path, write_range, CarveError, CarveHandler, CarvedFile, ExtractionContext,
};
use crate::scanner::NormalizedHit;

const ZIP_HEADER: &[u8] = b"PK\x03\x04";
const ZIP_EOCD: &[u8] = b"PK\x05\x06";

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

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
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
            validated = true;

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
            let mut file = File::create(&full_path)?;
            let mut md5 = md5::Context::new();
            let mut sha256 = Sha256::new();

            let (written, eof_truncated) = write_range(
                ctx,
                hit.global_offset,
                total_end,
                &mut file,
                &mut md5,
                &mut sha256,
            )?;
            bytes_written = written;
            if eof_truncated {
                truncated = true;
                errors.push("eof before EOCD end".to_string());
            }
            file.flush()?;

            if bytes_written < self.min_size {
                let _ = std::fs::remove_file(&full_path);
                return Ok(None);
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

            if let Some(parsed) = &eocd {
                if let Some(kind) = classify_zip(&full_path, parsed.cd_offset, parsed.cd_size) {
                    file_type = kind.file_type().to_string();
                    extension = kind.extension().to_string();
                    if file_type != self.file_type() {
                        if let Ok((new_path, new_rel)) =
                            output_path(ctx.output_root, &file_type, &extension, hit.global_offset)
                        {
                            if std::fs::rename(&full_path, &new_path).is_ok() {
                                rel_path = new_rel;
                                full_path = new_path;
                            }
                        }
                    }
                }
            }

            if let Some(allowed) = &self.allowed_kinds {
                if !allowed.contains(&file_type) {
                    let _ = std::fs::remove_file(&full_path);
                    return Ok(None);
                }
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

        let mut file = File::create(&full_path)?;
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

            if bytes_written == 0 && buf.len() >= ZIP_HEADER.len() {
                if &buf[..ZIP_HEADER.len()] != ZIP_HEADER {
                    let _ = std::fs::remove_file(&full_path);
                    return Ok(None);
                }
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
                    file.write_all(slice)?;
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
                        file.write_all(&extra)?;
                        md5.consume(&extra);
                        sha256.update(&extra);
                        bytes_written = bytes_written.saturating_add(extra.len() as u64);
                        extra_offset = extra_offset.saturating_add(extra.len() as u64);
                        remaining = remaining.saturating_sub(extra.len() as u64);
                    }
                }

                validated = true;
                break;
            }

            file.write_all(&buf)?;
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

        file.flush()?;

        if bytes_written < self.min_size {
            let _ = std::fs::remove_file(&full_path);
            return Ok(None);
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

        if validated {
            if let Some(parsed) = &eocd {
                if let Some(kind) = classify_zip(&full_path, parsed.cd_offset, parsed.cd_size) {
                    file_type = kind.file_type().to_string();
                    extension = kind.extension().to_string();
                    if file_type != self.file_type() {
                        if let Ok((new_path, new_rel)) =
                            output_path(ctx.output_root, &file_type, &extension, hit.global_offset)
                        {
                            if std::fs::rename(&full_path, &new_path).is_ok() {
                                rel_path = new_rel;
                            }
                        }
                    }
                }
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

fn find_eocd(
    ctx: &ExtractionContext,
    start: u64,
    max_size: u64,
) -> Result<Option<(u64, ZipEocd)>, CarveError> {
    let mut offset = start;
    let mut bytes_scanned = 0u64;
    let mut carry: Vec<u8> = Vec::new();
    let buf_size = 64 * 1024;

    loop {
        if max_size > 0 && bytes_scanned >= max_size {
            return Ok(None);
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
            return Ok(None);
        }
        buf.truncate(n);

        if bytes_scanned == 0 && buf.len() >= ZIP_HEADER.len() {
            if &buf[..ZIP_HEADER.len()] != ZIP_HEADER {
                return Ok(None);
            }
        }

        let mut search_buf = carry.clone();
        search_buf.extend_from_slice(&buf);
        let mut search_start = 0usize;
        while let Some(pos) = find_pattern(&search_buf[search_start..], ZIP_EOCD) {
            let absolute = search_start + pos;
            let eocd_offset = offset.saturating_sub(carry.len() as u64) + absolute as u64;
            if let Ok(parsed) = read_eocd(ctx, eocd_offset) {
                return Ok(Some((eocd_offset, parsed)));
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
    if &buf[0..4] != ZIP_EOCD {
        return Err(CarveError::Invalid(
            "zip eocd signature mismatch".to_string(),
        ));
    }
    let cd_size = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]) as u64;
    let cd_offset = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]) as u64;
    let comment_len = u16::from_le_bytes([buf[20], buf[21]]);
    Ok(ZipEocd {
        cd_offset,
        cd_size,
        comment_len,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ZipKind {
    Docx,
    Xlsx,
    Pptx,
}

impl ZipKind {
    fn file_type(self) -> &'static str {
        match self {
            ZipKind::Docx => "docx",
            ZipKind::Xlsx => "xlsx",
            ZipKind::Pptx => "pptx",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            ZipKind::Docx => "docx",
            ZipKind::Xlsx => "xlsx",
            ZipKind::Pptx => "pptx",
        }
    }
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

    let mut idx = 0usize;
    while idx + 46 <= buf.len() {
        if &buf[idx..idx + 4] != b"PK\x01\x02" {
            break;
        }
        let name_len = u16::from_le_bytes([buf[idx + 28], buf[idx + 29]]) as usize;
        let extra_len = u16::from_le_bytes([buf[idx + 30], buf[idx + 31]]) as usize;
        let comment_len = u16::from_le_bytes([buf[idx + 32], buf[idx + 33]]) as usize;
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
        idx = name_end + extra_len + comment_len;
    }

    None
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
    use super::{classify_zip, ZipCarveHandler, ZipKind};
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

    fn sample_zip_with_entry(name: &str) -> Vec<u8> {
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len() as u16;
        let mut out = Vec::new();

        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&[0x14, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]);
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]);
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(b"x");

        out.extend_from_slice(b"PK\x01\x02");
        out.extend_from_slice(&[0x14, 0x00]);
        out.extend_from_slice(&[0x14, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]);
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]);
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        out.extend_from_slice(name_bytes);

        let cd_size = 46 + name_bytes.len();
        let cd_offset = 30 + name_bytes.len() + 1;

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
        };
        let handler = ZipCarveHandler::new("zip".to_string(), 0, 1024, true, None);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "zip".to_string(),
            pattern_id: "zip_header".to_string(),
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
        };
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "zip".to_string(),
            pattern_id: "zip_header".to_string(),
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
}
