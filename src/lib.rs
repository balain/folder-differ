//! Core library for folder-differ: high-performance folder diffing and sync utilities.
//!
//! This crate provides modules for directory diffing, file hashing, synchronization actions, and progress reporting.

use std::fs::Metadata;
use std::path::Path;
use rustc_hash::FxHashMap;

/// Utility function for directory walking with ignore patterns.
pub fn get_dir_files_with_ignore(root: &Path, files: &mut FxHashMap<String, Metadata>, ignore_patterns: &[String]) {
    use ignore::WalkBuilder;
    let mut builder = WalkBuilder::new(root);
    for pat in ignore_patterns {
        builder.add_ignore(pat);
    }
    let walker = builder.build();
    for result in walker {
        if let Ok(entry) = result {
            let path = entry.path();
            if path.is_file() {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(rel_path) = path.strip_prefix(root) {
                        files.insert(rel_path.to_string_lossy().to_string(), meta);
                    }
                }
            }
        }
    }
}

pub mod diff;
pub mod sync;
pub mod hash;
pub mod progress; 