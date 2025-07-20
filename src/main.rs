use std::fs::{self, File, Metadata};
use std::sync::atomic::AtomicUsize;
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
    let mut buffer = [0u8; 32768]; // Increased buffer size for better I/O performance
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

fn run_synthetic_benchmark() {
    use std::time::Instant;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use indicatif::ProgressBar;

    let root = Path::new("/tmp/folder-differ-bench");
    let n_dirs = 100;
    let n_files_per_dir = 100;
    let file_size = 4096;

    // Clean up any previous benchmark tree
    let _ = fs::remove_dir_all(root);
    fs::create_dir_all(root).unwrap();
    for d in 0..n_dirs {
        let dir = root.join(format!("dir{:03}", d));
        fs::create_dir_all(&dir).unwrap();
        for f in 0..n_files_per_dir {
            let file = dir.join(format!("file{:03}.bin", f));
            std::fs::write(&file, vec![b'x'; file_size]).unwrap();
        }
    }
    println!("Synthetic tree created: {} dirs, {} files, {} bytes each", n_dirs, n_dirs * n_files_per_dir, file_size);
    let file_count = Arc::new(AtomicUsize::new(0));
    let dir_count = Arc::new(AtomicUsize::new(0));
    let active_tasks = Arc::new(AtomicUsize::new(1));
    let max_tasks = Arc::new(AtomicUsize::new(1));
    let pb = ProgressBar::hidden();
    let start = Instant::now();
    count_files_dirs(root, &file_count, &dir_count, &pb, &active_tasks, &max_tasks);
    let elapsed = start.elapsed();
    println!("Scan complete: files={}, dirs={}, time={:?}, max_parallel_tasks={}",
        file_count.load(Ordering::SeqCst),
        dir_count.load(Ordering::SeqCst),
        elapsed,
        max_tasks.load(Ordering::SeqCst));
    let _ = fs::remove_dir_all(root);
    println!("Synthetic benchmark finished and cleaned up.");
}

fn count_files_dirs(root: &Path, file_count: &std::sync::Arc<AtomicUsize>, dir_count: &std::sync::Arc<AtomicUsize>, pb: &indicatif::ProgressBar, active_tasks: &std::sync::Arc<std::sync::atomic::AtomicUsize>, max_tasks: &std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::Ordering;
    use std::fs;
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
                    let active_tasks = std::sync::Arc::clone(active_tasks);
                    let max_tasks = std::sync::Arc::clone(max_tasks);
                    s.spawn(move |_| {
                        let cur = active_tasks.fetch_add(1, Ordering::SeqCst) + 1;
                        max_tasks.fetch_max(cur, Ordering::SeqCst);
                        count_files_dirs(&path, &file_count, &dir_count, &pb, &active_tasks, &max_tasks);
                        active_tasks.fetch_sub(1, Ordering::SeqCst);
                    });
                } else {
                    file_count.fetch_add(1, Ordering::SeqCst);
                    pb.set_message(format!("Dirs: {}  Files: {}", dir_count.load(Ordering::SeqCst), file_count.load(Ordering::SeqCst)));
                }
            }
        });
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.contains(&"--synthetic-benchmark".to_string()) {
        run_synthetic_benchmark();
        return;
    }
    if args.len() < 3 {
        eprintln!("Usage: {} <left_dir> <right_dir> [--sync] [--dry-run] [--rollback] [--synthetic-benchmark]", args[0]);
        std::process::exit(1);
    }
    let left = Path::new(&args[1]);
    let right = Path::new(&args[2]);
    let do_sync = args.contains(&"--sync".to_string());
    let dry_run = args.contains(&"--dry-run".to_string());
    let do_rollback = args.contains(&"--rollback".to_string());

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

    let left_active_tasks = std::sync::Arc::new(AtomicUsize::new(1));
    let left_max_tasks = std::sync::Arc::new(AtomicUsize::new(1));
    let right_active_tasks = std::sync::Arc::new(AtomicUsize::new(1));
    let right_max_tasks = std::sync::Arc::new(AtomicUsize::new(1));
    rayon::join(
        || count_files_dirs(left, &left_file_count, &left_dir_count, &count_pb, &left_active_tasks, &left_max_tasks),
        || count_files_dirs(right, &right_file_count, &right_dir_count, &count_pb, &right_active_tasks, &right_max_tasks),
    );
    eprintln!("[DIAG] Max parallel tasks (left): {}", left_max_tasks.load(Ordering::SeqCst));
    eprintln!("[DIAG] Max parallel tasks (right): {}", right_max_tasks.load(Ordering::SeqCst));
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
        let mut files = HashMap::new();
        if let Ok(entries) = fs::read_dir(root) {
            let entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
            for entry in entries {
                let path = entry.path();
                let rel_path = path.strip_prefix(base).unwrap().to_string_lossy().to_string();
                if path.is_dir() {
                    dir_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    pb.set_message(format!("Dirs: {}  Files: {}", dir_count.load(std::sync::atomic::Ordering::SeqCst), file_count.load(std::sync::atomic::Ordering::SeqCst)));
                    pb.set_position((dir_count.load(std::sync::atomic::Ordering::SeqCst) + file_count.load(std::sync::atomic::Ordering::SeqCst)) as u64);
                    let sub_files = get_dir_files_progress(&path, base, file_count, dir_count, pb);
                    files.extend(sub_files);
                } else {
                    if let Ok(meta) = entry.metadata() {
                        file_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        pb.set_message(format!("Dirs: {}  Files: {}", dir_count.load(std::sync::atomic::Ordering::SeqCst), file_count.load(std::sync::atomic::Ordering::SeqCst)));
                        pb.set_position((dir_count.load(std::sync::atomic::Ordering::SeqCst) + file_count.load(std::sync::atomic::Ordering::SeqCst)) as u64);
                        files.insert(rel_path, meta);
                    }
                }
            }
        }
        files
    }

    let (left_files, right_files) = rayon::join(
        || get_dir_files_progress(left, left, &left_scan_file_count, &left_scan_dir_count, &scan_pb),
        || get_dir_files_progress(right, right, &right_scan_file_count, &right_scan_dir_count, &scan_pb),
    );
    scan_pb.finish_with_message("Scan complete");
    println!("About to start diff calculation...");
    let diffs: Vec<Diff> = {
        let all_paths: HashSet<_> = left_files.keys().chain(right_files.keys()).collect();
        let processed_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let total_files = all_paths.len();
        println!("Processing {} files in parallel...", total_files);
        
        all_paths.par_iter().filter_map(|path| {
            let count = processed_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if count % 1000 == 0 {
                print!("\rProcessed {} / {} files ({:.1}%)", count, total_files, (count as f64 / total_files as f64) * 100.0);
                std::io::stdout().flush().unwrap();
            }
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
                        // Only hash if sizes are the same but times differ
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
                        // Same size and time - assume identical, skip hashing
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
    println!("Scan duration: {:.2?}", scan_duration);
    println!("Files scanned: {}", total_files);
    println!("Differences found: {}", num_diffs);
    println!("Percent changed: {:.2}%", percent_changed);

    println!("About to print diffs...");
    if diffs.is_empty() {
        println!("No differences found.");
    } else {
        println!("Differences:");
        for diff in diffs.iter().take(10) {
            println!("Diff: {:?}", diff);
        }
        if diffs.len() > 10 {
            println!("...and {} more", diffs.len() - 10);
        }
    }
    println!("Done printing diffs.");
    println!("Scan duration: {:.2?}", scan_duration);
    println!("Files scanned: {}", total_files);
    println!("Differences found: {}", num_diffs);
    println!("Percent changed: {:.2}%", percent_changed);
}

