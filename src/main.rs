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



use std::fs::{OpenOptions};
use std::io::Write;
use std::collections::VecDeque;

#[derive(Debug, Clone)]
enum SyncAction {
    CopyLeftToRight(String),
    CopyRightToLeft(String),
    DeleteLeft(String),
    DeleteRight(String),
    Conflict(String),
    NoOp(String),
}

#[derive(Debug, Clone)]
struct SyncLogEntry {
    action: SyncAction,
    timestamp: SystemTime,
    details: String,
}

#[derive(Debug, Clone, Default)]
struct SyncLog {
    entries: Vec<SyncLogEntry>,
}

#[derive(Debug, Clone, Default)]
struct SyncState {
    last_synced: Option<SystemTime>,
    // Could store hashes, timestamps, etc for smarter conflict detection
}

fn plan_sync_actions(diffs: &[Diff], sync_mode: &str) -> Vec<SyncAction> {
    // Placeholder: build sync actions based on diffs and sync_mode
    diffs.iter().map(|diff| match &diff.diff_type {
        DiffType::OnlyInLeft => SyncAction::CopyLeftToRight(diff.path.clone()),
        DiffType::OnlyInRight => SyncAction::CopyRightToLeft(diff.path.clone()),
        DiffType::Different { .. } => SyncAction::CopyLeftToRight(diff.path.clone()), // Example policy
    }).collect()
}

fn log_sync_action(log: &mut SyncLog, action: &SyncAction, details: &str) {
    log.entries.push(SyncLogEntry {
        action: action.clone(),
        timestamp: SystemTime::now(),
        details: details.to_string(),
    });
}

fn save_sync_log(log: &SyncLog, path: &Path) {
    let log_path = path.join(".sync-log.txt");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        for entry in &log.entries {
            let _ = writeln!(file, "{:?} at {:?}: {}", entry.action, entry.timestamp, entry.details);
        }
    }
}

fn save_sync_state(state: &SyncState, path: &Path) {
    // Placeholder: serialize state to disk
    let _ = std::fs::write(path.join(".sync-state.json"), "{}\n");
}

fn rollback(log: &SyncLog, left: &Path, right: &Path) {
    // Placeholder: undo actions based on log
    println!("[ROLLBACK] Would undo {} actions", log.entries.len());
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <dir1> <dir2> [--sync <mode>] [--dry-run] [--rollback]", args[0]);
        std::process::exit(1);
    }
    let left = Path::new(&args[1]);
    let right = Path::new(&args[2]);
    let mut sync_mode = "none";
    let mut dry_run = false;
    let mut do_rollback = false;
    for arg in &args[3..] {
        match arg.as_str() {
            "--sync" => sync_mode = "one-way", // placeholder, could parse next arg
            "--dry-run" => dry_run = true,
            "--rollback" => do_rollback = true,
            _ => {}
        }
    }

    let diffs = compare_dirs(left, right);
    if do_rollback {
        // Load log and rollback
        let log = SyncLog::default(); // Placeholder: load from disk
        rollback(&log, left, right);
        return;
    }
    if sync_mode != "none" {
        let actions = plan_sync_actions(&diffs, sync_mode);
        let mut log = SyncLog::default();
        println!("Planned sync actions:");
        for action in &actions {
            println!("  {:?}", action);
        }
        if dry_run {
            println!("[DRY RUN] No changes made.");
        } else {
            // Placeholder: execute actions, log each
            for action in &actions {
                log_sync_action(&mut log, action, "executed");
            }
            save_sync_log(&log, left);
            save_sync_state(&SyncState::default(), left);
            println!("Sync complete. Log and state saved.");
        }
        return;
    }
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

