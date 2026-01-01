use std::fs::File;

use crate::carve::{
    output_path, CarveError, CarveHandler, CarveStream, CarvedFile, ExtractionContext,
};
use crate::scanner::NormalizedHit;

const GIF87A: &[u8] = b"GIF87a";
const GIF89A: &[u8] = b"GIF89a";

pub struct GifCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl GifCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for GifCarveHandler {
    fn file_type(&self) -> &str {
        "gif"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<CarvedFile>, CarveError> {
        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let file = File::create(&full_path)?;
        let mut stream = CarveStream::new(ctx.evidence, hit.global_offset, self.max_size, file);

        let mut validated = false;
        let mut truncated = false;
        let mut errors = Vec::new();

        let result: Result<(), CarveError> = (|| {
            let header = stream.read_exact(6)?;
            if header != GIF87A && header != GIF89A {
                return Err(CarveError::Invalid("gif header mismatch".to_string()));
            }

            let lsd = stream.read_exact(7)?;
            let packed = lsd[4];
            let gct_flag = (packed & 0b1000_0000) != 0;
            if gct_flag {
                let size_pow = (packed & 0b0000_0111) as u32;
                let gct_size = 3u64 * (1u64 << (size_pow + 1));
                stream.read_exact(gct_size as usize)?;
            }

            loop {
                let block_id = stream.read_exact(1)?[0];
                match block_id {
                    0x3B => {
                        validated = true;
                        break;
                    }
                    0x21 => {
                        stream.read_exact(1)?; // label
                        read_sub_blocks(&mut stream)?;
                    }
                    0x2C => {
                        let image_desc = stream.read_exact(9)?;
                        let packed = image_desc[8];
                        let lct_flag = (packed & 0b1000_0000) != 0;
                        if lct_flag {
                            let size_pow = (packed & 0b0000_0111) as u32;
                            let lct_size = 3u64 * (1u64 << (size_pow + 1));
                            stream.read_exact(lct_size as usize)?;
                        }
                        stream.read_exact(1)?; // LZW min code size
                        read_sub_blocks(&mut stream)?;
                    }
                    _ => return Err(CarveError::Invalid("gif block id invalid".to_string())),
                }
            }

            Ok(())
        })();

        if let Err(err) = result {
            match err {
                CarveError::Truncated | CarveError::Eof => {
                    truncated = true;
                    errors.push(err.to_string());
                }
                CarveError::Invalid(_msg) => {
                    let _ = std::fs::remove_file(&full_path);
                    return Ok(None);
                }
                other => return Err(other),
            }
        }

        let (size, md5_hex, sha256_hex) = stream.finish()?;
        if size < self.min_size {
            let _ = std::fs::remove_file(&full_path);
            return Ok(None);
        }

        let global_end = if size == 0 {
            hit.global_offset
        } else {
            hit.global_offset + size - 1
        };

        Ok(Some(CarvedFile {
            run_id: ctx.run_id.to_string(),
            file_type: self.file_type().to_string(),
            path: rel_path,
            extension: self.extension.clone(),
            global_start: hit.global_offset,
            global_end,
            size,
            md5: Some(md5_hex),
            sha256: Some(sha256_hex),
            validated,
            truncated,
            errors,
            pattern_id: Some(hit.pattern_id.clone()),
        }))
    }
}

fn read_sub_blocks(stream: &mut CarveStream) -> Result<(), CarveError> {
    loop {
        let size = stream.read_exact(1)?[0];
        if size == 0 {
            break;
        }
        stream.read_exact(size as usize)?;
    }
    Ok(())
}
