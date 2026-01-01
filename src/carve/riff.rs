//! Shared RIFF container parsing utilities.
//!
//! RIFF (Resource Interchange File Format) is used by WAV, AVI, and WebP.
//! This module provides common parsing functions.

use crate::carve::CarveError;

/// RIFF header magic bytes
pub const RIFF_MAGIC: &[u8; 4] = b"RIFF";

/// WAV form type
pub const WAVE_FORM: &[u8; 4] = b"WAVE";

/// AVI form type  
pub const AVI_FORM: &[u8; 4] = b"AVI ";

/// Parse RIFF header and return (form_type, declared_size).
///
/// The header is 12 bytes:
/// - Bytes 0-3: "RIFF"
/// - Bytes 4-7: Chunk size (little-endian u32) - size of everything after this field
/// - Bytes 8-11: Form type (e.g., "WAVE", "AVI ")
///
/// Total file size = chunk_size + 8
pub fn parse_riff_header(header: &[u8]) -> Result<([u8; 4], u64), CarveError> {
    if header.len() < 12 {
        return Err(CarveError::Invalid("riff header too short".to_string()));
    }

    if &header[0..4] != RIFF_MAGIC {
        return Err(CarveError::Invalid("riff magic mismatch".to_string()));
    }

    let chunk_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as u64;
    let total_size = chunk_size.saturating_add(8);

    let mut form_type = [0u8; 4];
    form_type.copy_from_slice(&header[8..12]);

    Ok((form_type, total_size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wav_header() {
        // RIFF + size (100) + WAVE
        let header = b"RIFF\x64\x00\x00\x00WAVE";
        let (form_type, size) = parse_riff_header(header).unwrap();
        assert_eq!(&form_type, WAVE_FORM);
        assert_eq!(size, 108); // 100 + 8
    }

    #[test]
    fn parse_avi_header() {
        // RIFF + size (1000) + AVI
        let header = b"RIFF\xe8\x03\x00\x00AVI ";
        let (form_type, size) = parse_riff_header(header).unwrap();
        assert_eq!(&form_type, AVI_FORM);
        assert_eq!(size, 1008); // 1000 + 8
    }

    #[test]
    fn rejects_invalid_magic() {
        let header = b"XXXX\x64\x00\x00\x00WAVE";
        assert!(parse_riff_header(header).is_err());
    }

    #[test]
    fn rejects_short_header() {
        let header = b"RIFF\x64\x00";
        assert!(parse_riff_header(header).is_err());
    }
}
