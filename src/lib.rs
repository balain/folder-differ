//! Core library for folder-differ: high-performance folder diffing and sync utilities.
//!
//! This crate provides modules for directory diffing, file hashing, synchronization actions, and progress reporting.

pub mod diff;
pub mod hash;
pub mod progress;
pub mod sync;

use rustc_hash::FxHashMap;
use std::fs::Metadata;
use std::path::Path;
use thiserror::Error;

/// The error type for all folder-differ library operations.
#[derive(Debug, Error)]
pub enum FolderDifferError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Walk error: {0}")]
    Walk(#[from] ignore::Error),
    #[error("Other error: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, FolderDifferError>;

/// Utility function for directory walking with ignore patterns.
pub fn get_dir_files_with_ignore(
    root: &Path,
    files: &mut FxHashMap<String, Metadata>,
    ignore_patterns: &[String],
) -> Result<()> {
    use ignore::WalkBuilder;
    let mut builder = WalkBuilder::new(root);
    for pat in ignore_patterns {
        builder.add_ignore(pat);
    }
    let walker = builder.build();
    for result in walker {
        let entry = result?;
        let path = entry.path();
        if path.is_file() {
            let meta = entry.metadata()?;
            if let Ok(rel_path) = path.strip_prefix(root) {
                files.insert(rel_path.to_string_lossy().to_string(), meta);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustc_hash::FxHashMap;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_get_dir_files_with_ignore_basic() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("foo.txt");
        File::create(&file_path).unwrap().write_all(b"abc").unwrap();

        let mut files = FxHashMap::default();
        get_dir_files_with_ignore(dir.path(), &mut files, &[]).unwrap();
        assert!(files.contains_key("foo.txt"));
    }
}
