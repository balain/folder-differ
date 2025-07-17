use std::env;
use std::fs::{self, File, Metadata};
use std::path::Path;
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

fn plan_sync_actions(diffs: &[Diff], _sync_mode: &str) -> Vec<SyncAction> {
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

fn save_sync_state(_state: &SyncState, path: &Path) {
    // Placeholder: serialize state to disk
    let _ = std::fs::write(path.join(".sync-state.json"), "{}\n");
}

fn backup_file(path: &Path) -> Option<std::path::PathBuf> {
    if path.exists() {
        let backup_path = path.with_extension("bak");
        std::fs::copy(path, &backup_path).ok()?;
        Some(backup_path)
    } else {
        None
    }
}

fn restore_file(backup_path: &Path, orig_path: &Path) -> bool {
    std::fs::copy(backup_path, orig_path).is_ok()
}

fn delete_file_with_backup(path: &Path) -> Option<std::path::PathBuf> {
    let backup = backup_file(path);
    std::fs::remove_file(path).ok()?;
    backup
}

fn perform_sync_action(action: &SyncAction, left: &Path, right: &Path, log: &mut SyncLog) {
    match action {
        SyncAction::CopyLeftToRight(rel_path) => {
            let src = left.join(rel_path);
            let dst = right.join(rel_path);
            if let Some(parent) = dst.parent() { let _ = std::fs::create_dir_all(parent); }
            let backup = backup_file(&dst);
            let res = std::fs::copy(&src, &dst);
            let msg = if let Ok(_) = res {
                format!("Copied {} to right. Backup: {:?}", rel_path, backup)
            } else {
                format!("FAILED to copy {} to right", rel_path)
            };
            log_sync_action(log, action, &msg);
        }
        SyncAction::CopyRightToLeft(rel_path) => {
            let src = right.join(rel_path);
            let dst = left.join(rel_path);
            if let Some(parent) = dst.parent() { let _ = std::fs::create_dir_all(parent); }
            let backup = backup_file(&dst);
            let res = std::fs::copy(&src, &dst);
            let msg = if let Ok(_) = res {
                format!("Copied {} to left. Backup: {:?}", rel_path, backup)
            } else {
                format!("FAILED to copy {} to left", rel_path)
            };
            log_sync_action(log, action, &msg);
        }
        SyncAction::DeleteLeft(rel_path) => {
            let path = left.join(rel_path);
            let backup = delete_file_with_backup(&path);
            let msg = if backup.is_some() {
                format!("Deleted {} from left. Backup: {:?}", rel_path, backup)
            } else {
                format!("FAILED to delete {} from left", rel_path)
            };
            log_sync_action(log, action, &msg);
        }
        SyncAction::DeleteRight(rel_path) => {
            let path = right.join(rel_path);
            let backup = delete_file_with_backup(&path);
            let msg = if backup.is_some() {
                format!("Deleted {} from right. Backup: {:?}", rel_path, backup)
            } else {
                format!("FAILED to delete {} from right", rel_path)
            };
            log_sync_action(log, action, &msg);
        }
        SyncAction::Conflict(rel_path) => {
            let msg = format!("Conflict on {}. Manual resolution required.", rel_path);
            log_sync_action(log, action, &msg);
        }
        SyncAction::NoOp(rel_path) => {
            let msg = format!("No operation for {}.", rel_path);
            log_sync_action(log, action, &msg);
        }
    }
}

fn rollback(log: &SyncLog, left: &Path, right: &Path) {
    for entry in log.entries.iter().rev() {
        match &entry.action {
            SyncAction::CopyLeftToRight(rel_path) => {
                let dst = right.join(rel_path);
                let backup = dst.with_extension("bak");
                if backup.exists() {
                    restore_file(&backup, &dst);
                    let _ = std::fs::remove_file(&backup);
                } else {
                    let _ = std::fs::remove_file(&dst);
                }
                println!("Rolled back CopyLeftToRight: {}", rel_path);
            }
            SyncAction::CopyRightToLeft(rel_path) => {
                let dst = left.join(rel_path);
                let backup = dst.with_extension("bak");
                if backup.exists() {
                    restore_file(&backup, &dst);
                    let _ = std::fs::remove_file(&backup);
                } else {
                    let _ = std::fs::remove_file(&dst);
                }
                println!("Rolled back CopyRightToLeft: {}", rel_path);
            }
            SyncAction::DeleteLeft(rel_path) => {
                let orig = left.join(rel_path);
                let backup = orig.with_extension("bak");
                if backup.exists() {
                    restore_file(&backup, &orig);
                    let _ = std::fs::remove_file(&backup);
                }
                println!("Rolled back DeleteLeft: {}", rel_path);
            }
            SyncAction::DeleteRight(rel_path) => {
                let orig = right.join(rel_path);
                let backup = orig.with_extension("bak");
                if backup.exists() {
                    restore_file(&backup, &orig);
                    let _ = std::fs::remove_file(&backup);
                }
                println!("Rolled back DeleteRight: {}", rel_path);
            }
            SyncAction::Conflict(rel_path) | SyncAction::NoOp(rel_path) => {
                println!("No rollback for action on {}", rel_path);
            }
        }
    }
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

    use std::time::Instant;
    use indicatif::{ProgressBar, ProgressStyle, ProgressDrawTarget};
    let scan_start = Instant::now();
    // PHASE 1: Count files and directories (with progress bar)
    let count_pb = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr());
    count_pb.set_style(ProgressStyle::with_template("[Counting {elapsed_precise}] {msg}").unwrap());
    // Use separate counters for left and right
    use std::sync::atomic::{AtomicUsize, Ordering};
    let left_file_count = std::sync::Arc::new(AtomicUsize::new(0));
    let left_dir_count = std::sync::Arc::new(AtomicUsize::new(0));
    let right_file_count = std::sync::Arc::new(AtomicUsize::new(0));
    let right_dir_count = std::sync::Arc::new(AtomicUsize::new(0));

    fn count_files_dirs(root: &Path, file_count: &std::sync::Arc<AtomicUsize>, dir_count: &std::sync::Arc<AtomicUsize>, pb: &ProgressBar) {
        if let Ok(entries) = fs::read_dir(root) {
            let entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
            rayon::scope(|s| {
                for entry in &entries {
                    let path = entry.path();
                    if path.is_dir() {
                        dir_count.fetch_add(1, Ordering::SeqCst);
                        pb.set_message(format!("Dirs: {}  Files: {}", dir_count.load(Ordering::SeqCst), file_count.load(Ordering::SeqCst)));
                        let file_count = std::sync::Arc::clone(file_count);
                        let dir_count = std::sync::Arc::clone(dir_count);
                        let pb = pb.clone();
                        s.spawn(move |_| {
                            count_files_dirs(&path, &file_count, &dir_count, &pb);
                        });
                    } else {
                        file_count.fetch_add(1, Ordering::SeqCst);
                        pb.set_message(format!("Dirs: {}  Files: {}", dir_count.load(Ordering::SeqCst), file_count.load(Ordering::SeqCst)));
                    }
                }
            });
        }
    }
    rayon::join(
        || count_files_dirs(left, &left_file_count, &left_dir_count, &count_pb),
        || count_files_dirs(right, &right_file_count, &right_dir_count, &count_pb),
    );
    let file_total = left_file_count.load(Ordering::SeqCst) + right_file_count.load(Ordering::SeqCst);
    let dir_total = left_dir_count.load(Ordering::SeqCst) + right_dir_count.load(Ordering::SeqCst);
    count_pb.finish_with_message("Counting complete");
    let scan_total = file_total + dir_total;

    // PHASE 2: Scan with percent-complete progress bar
    let scan_pb = ProgressBar::with_draw_target(Some(scan_total as u64), ProgressDrawTarget::stderr());
    scan_pb.set_style(ProgressStyle::with_template("[{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {percent}% {msg}")
        .unwrap());
    // Use separate counters for left and right
    let left_scan_file_count = std::sync::Arc::new(AtomicUsize::new(0));
    let left_scan_dir_count = std::sync::Arc::new(AtomicUsize::new(0));
    let right_scan_file_count = std::sync::Arc::new(AtomicUsize::new(0));
    let right_scan_dir_count = std::sync::Arc::new(AtomicUsize::new(0));
    fn get_dir_files_progress(root: &Path, base: &Path, file_count: &std::sync::Arc<AtomicUsize>, dir_count: &std::sync::Arc<AtomicUsize>, pb: &ProgressBar) -> HashMap<String, Metadata> {
        use std::sync::{Arc, Mutex};
        let mut files = HashMap::new();
        if let Ok(entries) = fs::read_dir(root) {
            let entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
            let sub_maps = Arc::new(Mutex::new(Vec::new()));
            rayon::scope(|s| {
                for entry in &entries {
                    let path = entry.path();
                    let rel_path = path.strip_prefix(base).unwrap().to_string_lossy().to_string();
                    if path.is_dir() {
                        dir_count.fetch_add(1, Ordering::SeqCst);
                        pb.set_message(format!("Dirs: {}  Files: {}", dir_count.load(Ordering::SeqCst), file_count.load(Ordering::SeqCst)));
                        pb.set_position((dir_count.load(Ordering::SeqCst) + file_count.load(Ordering::SeqCst)) as u64);
                        let file_count = Arc::clone(file_count);
                        let dir_count = Arc::clone(dir_count);
                        let pb = pb.clone();
                        let base = base.to_path_buf();
                        let sub_maps = Arc::clone(&sub_maps);
                        s.spawn(move |_| {
                            let map = get_dir_files_progress(&path, &base, &file_count, &dir_count, &pb);
                            sub_maps.lock().unwrap().push(map);
                        });
                    } else {
                        if let Ok(meta) = entry.metadata() {
                            file_count.fetch_add(1, Ordering::SeqCst);
                            pb.set_message(format!("Dirs: {}  Files: {}", dir_count.load(Ordering::SeqCst), file_count.load(Ordering::SeqCst)));
                            pb.set_position((dir_count.load(Ordering::SeqCst) + file_count.load(Ordering::SeqCst)) as u64);
                            files.insert(rel_path, meta);
                        }
                    }
                }
            });
            for map in sub_maps.lock().unwrap().drain(..) {
                files.extend(map);
            }
        }
        files
    }
    let (left_files, right_files) = rayon::join(
        || get_dir_files_progress(left, left, &left_scan_file_count, &left_scan_dir_count, &scan_pb),
        || get_dir_files_progress(right, right, &right_scan_file_count, &right_scan_dir_count, &scan_pb),
    );
    scan_pb.finish_with_message("Scan complete");
    let diffs: Vec<Diff> = {
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
                            diff_type: DiffType::Different { left_size, right_size, left_time, right_time },
                        })
                    } else if left_time != right_time {
                        // Hash and compare
                        let left_hash = hash_file(&left.join(*path));
                        let right_hash = hash_file(&right.join(*path));
                        if left_hash != right_hash {
                            Some(Diff {
                                path: (*path).clone(),
                                diff_type: DiffType::Different { left_size, right_size, left_time, right_time },
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
    };
    let scan_duration = scan_start.elapsed();
    let total_files: usize = {
        let mut left_files = HashSet::new();
        let mut right_files = HashSet::new();
        fn collect_file_names(root: &Path, base: &Path, files: &mut HashSet<String>) {
            if let Ok(entries) = fs::read_dir(root) {
                entries.filter_map(|e| e.ok()).for_each(|entry| {
                    let path = entry.path();
                    let rel_path = path.strip_prefix(base).unwrap().to_string_lossy().to_string();
                    if path.is_dir() {
                        collect_file_names(&path, base, files);
                    } else {
                        files.insert(rel_path);
                    }
                });
            }
        }
        collect_file_names(left, left, &mut left_files);
        collect_file_names(right, right, &mut right_files);
        left_files.union(&right_files).count()
    };
    let num_diffs = diffs.len();
    let percent_changed = if total_files > 0 {
        (num_diffs as f64 / total_files as f64) * 100.0
    } else { 0.0 };

    if do_rollback {
        // Load log and rollback
        let log = SyncLog::default(); // Placeholder: load from disk
        rollback(&log, left, right);
        return;
    }
    if sync_mode != "none" {
        let actions = plan_sync_actions(&diffs, sync_mode);
        println!("Planned sync actions:");
        for action in &actions {
            println!("  {:?}", action);
        }
        if dry_run {
            println!("[DRY RUN] No changes made.");
        } else {
            // Parallel execution of sync actions
            use std::sync::{Arc, Mutex};
            use indicatif::{ProgressBar, ProgressStyle};
            let log = Arc::new(Mutex::new(SyncLog::default()));
            let pb = Arc::new(ProgressBar::with_draw_target(Some(actions.len() as u64), indicatif::ProgressDrawTarget::stderr()));
            pb.set_style(ProgressStyle::with_template("[{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}").unwrap());
            actions.par_iter().for_each(|action| {
                let mut thread_log = SyncLog::default();
                perform_sync_action(action, left, right, &mut thread_log);
                let mut main_log = log.lock().unwrap();
                main_log.entries.extend(thread_log.entries);
                pb.inc(1);
            });
            pb.finish_with_message("Done");
            let log = Arc::try_unwrap(log).unwrap().into_inner().unwrap();
            save_sync_log(&log, left);
            save_sync_state(&SyncState::default(), left);
            println!("Sync complete. Log and state saved.");
        }
        println!("Scan duration: {:.2?}", scan_duration);
        println!("Files scanned: {}", total_files);
        println!("Differences found: {}", num_diffs);
        println!("Percent changed: {:.2}%", percent_changed);
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
    println!("Scan duration: {:.2?}", scan_duration);
    println!("Files scanned: {}", total_files);
    println!("Differences found: {}", num_diffs);
    println!("Percent changed: {:.2}%", percent_changed);
}

