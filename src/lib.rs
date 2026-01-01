//! # fastcarve
//!
//! High-speed forensic file and artefact carver with optional GPU acceleration.
//!
//! This crate provides tools for extracting files and forensic artefacts from
//! disk images and raw evidence sources.

pub mod cli;
pub mod checkpoint;
pub mod config;
pub mod constants;
pub mod error;
pub mod evidence;
pub mod chunk;
pub mod scanner;
pub mod strings;
pub mod carve;
pub mod metadata;
pub mod parsers;
pub mod logging;
pub mod util;
pub mod entropy;
pub mod pipeline;
