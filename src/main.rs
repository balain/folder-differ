use std::env;
use std::fs::{self, File, Metadata};
use std::path::{Path, PathBuf};
use std::collections::{HashMap, HashSet};
use std::time::SystemTime;
use sha2::{Sha256, Digest};
use std::io::{Read, BufReader};
use rayon::prelude::*;

#[derive(Debug)]
enum DiffType {
    OnlyInLeft,
    OnlyInRight,
    Different {
        left_size: u64,
        right_size: u64,
        left_time: Option<SystemTime>,
        right_time: Option<SystemTime>,
    },
}

#[derive(Debug)]
struct Diff {
    path: String,
    diff_type: DiffType,
}

fn get_dir_files(root: &Path, base: &Path, files: &mut HashMap<String, Metadata>) {
    if let Ok(entries) = fs::read_dir(root) {
        entries.filter_map(|e| e.ok()).for_each(|entry| {
            let path = entry.path();
            let rel_path = path.strip_prefix(base).unwrap().to_string_lossy().to_string();
            if path.is_dir() {
                get_dir_files(&path, base, files);
            } else {
                if let Ok(meta) = entry.metadata() {
                    files.insert(rel_path, meta);
                }
            }
        });
    }
}


fn hash_file(path: &Path) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = reader.read(&mut buffer).ok()?;
        if n == 0 { break; }
        hasher.update(&buffer[..n]);
    }
    Some(hasher.finalize().to_vec())
}

fn compare_dirs(left: &Path, right: &Path) -> Vec<Diff> {
    let mut left_files: HashMap<String, Metadata> = HashMap::new();
    let mut right_files: HashMap<String, Metadata> = HashMap::new();

    rayon::join(
        || get_dir_files(left, left, &mut left_files),
        || get_dir_files(right, right, &mut right_files),
    );

    let all_paths: HashSet<_> = left_files.keys().chain(right_files.keys()).collect();
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
                    // Same size, different time: hash both
                    let left_path = left.join(*path);
                    let right_path = right.join(*path);
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



fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <dir1> <dir2>", args[0]);
        std::process::exit(1);
    }
    let left = Path::new(&args[1]);
    let right = Path::new(&args[2]);
    let diffs = compare_dirs(left, right);
    if diffs.is_empty() {
        println!("No differences found.");
    } else {
        println!("Differences:");
        for diff in diffs {
            match &diff.diff_type {
                DiffType::Different { left_size, right_size, left_time, right_time } => {
                    let newer = match (left_time, right_time) {
                        (Some(lt), Some(rt)) => {
                            if lt > rt {
                                "left is newer"
                            } else if rt > lt {
                                "right is newer"
                            } else {
                                "same time"
                            }
                        },
                        _ => "unknown",
                    };
                    let larger = if left_size > right_size {
                        "left is larger"
                    } else if right_size > left_size {
                        "right is larger"
                    } else {
                        "same size"
                    };
                    println!(
                        "Different: {}\n  left:  size={}  time={:?}\n  right: size={}  time={:?}\n  {} | {}",
                        diff.path, left_size, left_time, right_size, right_time, newer, larger
                    );
                }
                DiffType::OnlyInLeft => {
                    println!("Only in left: {}", diff.path);
                }
                DiffType::OnlyInRight => {
                    println!("Only in right: {}", diff.path);
                }
            }
        }
    }
}
