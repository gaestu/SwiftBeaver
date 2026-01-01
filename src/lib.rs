//! # fastcarve
//!
//! High-speed forensic file and artefact carver with optional GPU acceleration.
//!
//! This crate provides tools for extracting files and forensic artefacts from
//! disk images and raw evidence sources.

pub mod carve;
pub mod checkpoint;
pub mod chunk;
pub mod cli;
pub mod config;
pub mod constants;
pub mod entropy;
pub mod error;
pub mod evidence;
pub mod logging;
pub mod metadata;
pub mod parsers;
pub mod pipeline;
pub mod scanner;
pub mod strings;
pub mod util;
