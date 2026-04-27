use crate::carve::{
    CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext, PendingCarve,
    PreValidation, output_path, riff,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const PRIMARY_CHUNKS: [&[u8; 4]; 3] = [b"VP8 ", b"VP8L", b"VP8X"];

pub struct WebpCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl WebpCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for WebpCarveHandler {
    fn file_type(&self) -> &str {
        "webp"
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
        let mut buf = [0u8; 16];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < buf.len() {
            return Ok(PreValidation::Reject("truncated header".to_string()));
        }
        let total_size = match parse_webp_riff_header(&buf[0..12]) {
            Ok(total_size) => total_size,
            Err(CarveError::Invalid(reason)) => return Ok(PreValidation::Reject(reason)),
            Err(err) => return Err(err),
        };
        if total_size < 20 {
            return Ok(PreValidation::Reject(
                "webp RIFF size too small".to_string(),
            ));
        }
        if self.max_size > 0 && total_size > self.max_size {
            return Ok(PreValidation::Reject(
                "webp RIFF size exceeds max_size".to_string(),
            ));
        }
        if !is_primary_chunk(&buf[12..16]) {
            return Ok(PreValidation::Reject(
                "webp first chunk fourcc invalid".to_string(),
            ));
        }
        Ok(PreValidation::Proceed)
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
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
            let header = stream.read_exact(12)?;
            let total_size = parse_webp_riff_header(&header)?;
            if total_size < 20 {
                return Err(CarveError::Invalid("webp size invalid".to_string()));
            }
            if self.max_size > 0 && total_size > self.max_size {
                return Err(CarveError::Invalid(
                    "webp RIFF size exceeds max_size".to_string(),
                ));
            }

            consume_webp_chunks(&mut stream, total_size)?;
            validated = true;
            Ok(total_size)
        })();

        if let Err(err) = result {
            match err {
                CarveError::Eof => {
                    truncated = true;
                    errors.push("eof before WebP RIFF end".to_string());
                }
                CarveError::Truncated => {
                    truncated = true;
                    errors.push("max_size reached before WebP RIFF end".to_string());
                }
                CarveError::Invalid(_msg) => {
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

        let global_end = if size == 0 {
            hit.global_offset
        } else {
            hit.global_offset + size - 1
        };

        Ok(Some(PendingCarve::new(
            CarvedFile {
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
            },
            writer,
        )))
    }
}

fn is_primary_chunk(fourcc: &[u8]) -> bool {
    PRIMARY_CHUNKS
        .iter()
        .any(|chunk| fourcc == chunk.as_slice())
}

fn parse_webp_riff_header(header: &[u8]) -> Result<u64, CarveError> {
    let (form_type, total_size) = riff::parse_riff_header(header)?;
    if &form_type != riff::WEBP_FORM {
        return Err(CarveError::Invalid("webp WEBP marker mismatch".to_string()));
    }
    Ok(total_size)
}

fn padded_chunk_size(size: u64) -> u64 {
    size + (size % 2)
}

fn consume_webp_chunks(stream: &mut CarveStream<'_>, total_size: u64) -> Result<(), CarveError> {
    let mut remaining = total_size.saturating_sub(12);
    let mut chunk_index = 0usize;

    while remaining > 0 {
        if remaining < 8 {
            return Err(CarveError::Invalid(
                "webp trailing bytes outside chunk".to_string(),
            ));
        }

        let chunk_header = stream.read_exact(8)?;
        let fourcc = &chunk_header[0..4];
        if chunk_index == 0 && !is_primary_chunk(fourcc) {
            return Err(CarveError::Invalid(
                "webp first chunk fourcc invalid".to_string(),
            ));
        }

        let chunk_size = u32::from_le_bytes([
            chunk_header[4],
            chunk_header[5],
            chunk_header[6],
            chunk_header[7],
        ]) as u64;
        let padded_size = padded_chunk_size(chunk_size);
        remaining -= 8;
        if padded_size > remaining {
            return Err(CarveError::Invalid(
                "webp chunk exceeds RIFF container".to_string(),
            ));
        }

        if padded_size > 0 {
            stream.consume_remaining(padded_size)?;
        }
        remaining -= padded_size;
        chunk_index += 1;
    }

    Ok(())
}
