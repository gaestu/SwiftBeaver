//! # Constants Module
//!
//! Centralized constants used throughout the fastcarve crate.
//! This avoids magic numbers scattered across the codebase.

/// Default I/O buffer size for reading/writing file data (64 KiB)
pub const DEFAULT_IO_BUFFER_SIZE: usize = 64 * 1024;

/// One Mebibyte in bytes
pub const MIB: u64 = 1024 * 1024;

/// One Kibibyte in bytes
pub const KIB: u64 = 1024;

/// Default channel capacity multiplier for workers
pub const CHANNEL_CAPACITY_MULTIPLIER: usize = 2;

/// Minimum channel capacity
pub const MIN_CHANNEL_CAPACITY: usize = 1;

/// Default chunk size in MiB for scanning
pub const DEFAULT_CHUNK_SIZE_MIB: u64 = 512;

/// Default overlap in KiB for chunk scanning
pub const DEFAULT_OVERLAP_KIB: u64 = 64;
