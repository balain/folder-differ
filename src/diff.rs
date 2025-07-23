//! Diffing logic and types for folder-differ

use std::fs::Metadata;
use std::path::Path;
use std::time::SystemTime;
use rustc_hash::{FxHashMap, FxHashSet};
use rayon::prelude::*;
use crate::hash::{hash_file, compare_small_files};
use super::get_dir_files_with_ignore;

/// The type of difference between two files or directories.
#[derive(Debug)]
pub enum DiffType {
    OnlyInLeft,
    OnlyInRight,
    Different {
        left_size: u64,
        right_size: u64,
        left_time: Option<SystemTime>,
        right_time: Option<SystemTime>,
    },
}

/// Represents a difference found between two directories.
#[derive(Debug)]
pub struct Diff {
    pub path: String,
    pub diff_type: DiffType,
}

/// Compares two directories and returns a list of differences.
///
/// # Arguments
/// * `left` - Path to the left directory
/// * `right` - Path to the right directory
///
/// # Returns
/// A vector of `Diff` representing the differences found.
pub fn compare_dirs(left: &Path, right: &Path) -> Vec<Diff> {
    let mut left_files: FxHashMap<String, Metadata> = FxHashMap::default();
    let mut right_files: FxHashMap<String, Metadata> = FxHashMap::default();

    rayon::join(
        || get_dir_files_with_ignore(left, &mut left_files, &[]),
        || get_dir_files_with_ignore(right, &mut right_files, &[]),
    );

    let all_paths: FxHashSet<_> = left_files.keys().chain(right_files.keys()).collect();
    all_paths.par_iter().filter_map(|path| {
        match (left_files.get(*path), right_files.get(*path)) {
            (Some(left_meta), Some(right_meta)) => {
                let left_size = left_meta.len();
                let right_size = right_meta.len();
                let left_time = left_meta.modified().ok();
                let right_time = right_meta.modified().ok();
                if left_size != right_size {
                    Some(Diff {
                        path: (*path).clone(),
                        diff_type: DiffType::Different {
                            left_size,
                            right_size,
                            left_time,
                            right_time,
                        },
                    })
                } else if left_time != right_time {
                    let left_path = left.join(*path);
                    let right_path = right.join(*path);
                    if left_size < 1024 {
                        if let Some(are_equal) = compare_small_files(&left_path, &right_path) {
                            if !are_equal {
                                Some(Diff {
                                    path: (*path).clone(),
                                    diff_type: DiffType::Different {
                                        left_size,
                                        right_size,
                                        left_time,
                                        right_time,
                                    },
                                })
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        let left_hash = hash_file(&left_path);
                        let right_hash = hash_file(&right_path);
                        if left_hash != right_hash {
                            Some(Diff {
                                path: (*path).clone(),
                                diff_type: DiffType::Different {
                                    left_size,
                                    right_size,
                                    left_time,
                                    right_time,
                                },
                            })
                        } else {
                            None
                        }
                    }
                } else {
                    None
                }
            }
            (Some(_), None) => {
                Some(Diff {
                    path: (*path).clone(),
                    diff_type: DiffType::OnlyInLeft,
                })
            }
            (None, Some(_)) => {
                Some(Diff {
                    path: (*path).clone(),
                    diff_type: DiffType::OnlyInRight,
                })
            }
            (None, None) => None,
        }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs::{self, File};
    use std::io::Write;
    use std::path::Path;

    fn write_file(path: &Path, content: &[u8]) {
        let mut file = File::create(path).unwrap();
        file.write_all(content).unwrap();
    }

    #[test]
    fn test_identical_dirs() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();
        write_file(&dir1.path().join("a.txt"), b"hello");
        write_file(&dir2.path().join("a.txt"), b"hello");

        let diffs = super::compare_dirs(dir1.path(), dir2.path());
        assert!(diffs.is_empty(), "No diffs expected for identical dirs");
    }

    #[test]
    fn test_file_only_in_left() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();
        write_file(&dir1.path().join("a.txt"), b"hello");

        let diffs = super::compare_dirs(dir1.path(), dir2.path());
        assert_eq!(diffs.len(), 1);
        assert!(matches!(diffs[0].diff_type, super::DiffType::OnlyInLeft));
    }

    #[test]
    fn test_file_content_diff() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();
        write_file(&dir1.path().join("a.txt"), b"hello");
        write_file(&dir2.path().join("a.txt"), b"world");

        let diffs = super::compare_dirs(dir1.path(), dir2.path());
        assert_eq!(diffs.len(), 1);
        assert!(matches!(diffs[0].diff_type, super::DiffType::Different { .. }));
    }
} 