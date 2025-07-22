use std::env;
use std::fs::{self, File, Metadata};
use std::sync::atomic::AtomicUsize;
use std::path::Path;
use std::time::SystemTime;
use sha2::{Sha256, Digest};
use std::io::{Read, BufReader};
use rayon::prelude::*;
use memmap2::Mmap;
use blake3;
use ignore::{WalkBuilder, WalkState, DirEntry};
use rustc_hash::{FxHashMap, FxHashSet};
use std::io::{BufWriter, Write};
use std::time::Instant;
use num_cpus;
use std::sync::{Arc, Mutex};
use jwalk::WalkDirGeneric;
use jwalk::Parallelism;
use std::io::Seek;

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

// Replace get_dir_files and related logic with ignore::WalkBuilder and FxHashMap/FxHashSet
fn get_dir_files_with_ignore(root: &Path, files: &mut FxHashMap<String, Metadata>, ignore_patterns: &[String]) {
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

// Hash sampling for large files: hash only first and last 64KB for files > 100MB
fn hash_sampled_file(path: &Path) -> Option<Vec<u8>> {
    const SAMPLE_SIZE: usize = 64 * 1024; // 64KB
    const MIN_SIZE: u64 = 100 * 1024 * 1024; // 100MB
    let file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let file_size = metadata.len();
    if file_size < MIN_SIZE {
        return None;
    }
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; SAMPLE_SIZE];
    // Hash first 64KB
    let mut reader = BufReader::new(&file);
    let n = reader.read(&mut buf).ok()?;
    hasher.update(&buf[..n]);
    // Hash last 64KB
    if file_size > SAMPLE_SIZE as u64 {
        let mut file = File::open(path).ok()?;
        file.seek(std::io::SeekFrom::End(-(SAMPLE_SIZE as i64))).ok()?;
        let n = file.read(&mut buf).ok()?;
        hasher.update(&buf[..n]);
    }
    Some(hasher.finalize().as_bytes().to_vec())
}

// jwalk-based fast parallel directory traversal
fn get_dir_files_jwalk(root: &Path) -> FxHashMap<String, Metadata> {
    let mut files = FxHashMap::default();
    for entry in jwalk::WalkDir::new(root)
        .parallelism(Parallelism::RayonNewPool(8))
        .skip_hidden(false)
    {
        if let Ok(dir_entry) = entry {
            if dir_entry.file_type().is_file() {
                let rel_path = dir_entry.path().strip_prefix(root).unwrap().to_string_lossy().to_string();
                if let Ok(meta) = dir_entry.metadata() {
                    files.insert(rel_path, meta);
                }
            }
        }
    }
    files
}

fn hash_file(path: &Path) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let file_size = metadata.len();
    if let Some(sampled_hash) = hash_sampled_file(path) {
        return Some(sampled_hash);
    }
    if file_size < 1024 {
        return hash_small_file(path);
    }
    if file_size > 1024 * 1024 {
        return hash_large_file_blake3(path);
    }
    hash_medium_file_blake3(path)
}

fn hash_small_file(path: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let mut content = Vec::new();
    file.read_to_end(&mut content).ok()?;
    let hash = blake3::hash(&content);
    Some(hash.as_bytes().to_vec())
}

fn hash_medium_file_blake3(path: &Path) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 32768];
    loop {
        let n = reader.read(&mut buffer).ok()?;
        if n == 0 { break; }
        hasher.update(&buffer[..n]);
    }
    Some(hasher.finalize().as_bytes().to_vec())
}

fn hash_large_file_blake3(path: &Path) -> Option<Vec<u8>> {
    // BLAKE3 natively parallelizes hashing of large files
    let file = File::open(path).ok()?;
    let mmap = unsafe { Mmap::map(&file).ok()? };
    // Use the default parallel hash API
    let hash = blake3::Hasher::new().update(&mmap).finalize();
    Some(hash.as_bytes().to_vec())
}

fn compare_small_files(left_path: &Path, right_path: &Path) -> Option<bool> {
    let mut left_content = Vec::new();
    let mut right_content = Vec::new();
    
    File::open(left_path).ok()?.read_to_end(&mut left_content).ok()?;
    File::open(right_path).ok()?.read_to_end(&mut right_content).ok()?;
    
    Some(left_content == right_content)
}

fn compare_dirs(left: &Path, right: &Path) -> Vec<Diff> {
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

fn print_usage(program: &str) {
    println!("Usage: {} <left_dir> <right_dir> [--threads N] [--sync] [--dry-run] [--rollback] [--synthetic-benchmark]", program);
    println!("\nOptions:");
    println!("  --threads N              Set number of threads for parallelism (default: 2x logical CPUs)");
    println!("  --sync                   Plan and perform sync actions (copy/delete files)");
    println!("  --dry-run                Show planned sync actions without making changes");
    println!("  --rollback               Roll back the last sync operation using backups");
    println!("  --synthetic-benchmark    Run a synthetic benchmark (creates and scans a large fake tree)");
    println!("  --help                   Show this help message");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage(&args[0]);
        return;
    }
    // Thread count CLI option
    let mut thread_count: Option<usize> = None;
    let mut left_dir_arg = None;
    let mut right_dir_arg = None;
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--threads" && i + 1 < args.len() {
            if let Ok(n) = args[i + 1].parse::<usize>() {
                thread_count = Some(n);
            }
            i += 2;
        } else if left_dir_arg.is_none() {
            left_dir_arg = Some(args[i].clone());
            i += 1;
        } else if right_dir_arg.is_none() {
            right_dir_arg = Some(args[i].clone());
            i += 1;
        } else {
            i += 1;
        }
    }
    if left_dir_arg.is_none() || right_dir_arg.is_none() {
        print_usage(&args[0]);
        std::process::exit(1);
    }
    let left_dir = left_dir_arg.unwrap();
    let right_dir = right_dir_arg.unwrap();
    let left = Path::new(&left_dir);
    let right = Path::new(&right_dir);

    // Sensible default: 2x logical CPUs for I/O bound
    let default_threads = num_cpus::get() * 2;
    let num_threads = thread_count.unwrap_or(default_threads);
    rayon::ThreadPoolBuilder::new().num_threads(num_threads).build_global().unwrap();
    eprintln!("[CONFIG] Using {} threads for Rayon pool", num_threads);

    if args.contains(&"--synthetic-benchmark".to_string()) {
        run_synthetic_benchmark();
        return;
    }
    let do_sync = args.contains(&"--sync".to_string());
    let dry_run = args.contains(&"--dry-run".to_string());
    let do_rollback = args.contains(&"--rollback".to_string());

    // Output file logic
    let left_name = left.file_name().and_then(|n| n.to_str()).unwrap_or("left");
    let right_name = right.file_name().and_then(|n| n.to_str()).unwrap_or("right");
    let output_dir = Path::new("./output");
    std::fs::create_dir_all(output_dir).ok();
    let output_path = output_dir.join(format!("{}_vs_{}.txt", left_name, right_name));
    let output_file = File::create(&output_path).expect("Failed to create output file");
    let mut writer = BufWriter::new(output_file);

    // Timing: start
    let total_start = Instant::now();

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

    let phase1_time = scan_start.elapsed();
    eprintln!("[BENCH] Phase 1 (counting) duration: {:.2?}", phase1_time);

    // PHASE 2: Scan with percent-complete progress bar using jwalk
    let left_total = left_file_count.load(Ordering::SeqCst) + left_dir_count.load(Ordering::SeqCst);
    let right_total = right_file_count.load(Ordering::SeqCst) + right_dir_count.load(Ordering::SeqCst);

    let phase2_start = Instant::now();
    let left_scan_pb = ProgressBar::new(left_total as u64);
    left_scan_pb.set_style(ProgressStyle::with_template("[Left {elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {percent}% {msg}").unwrap());
    let mut left_files = FxHashMap::default();
    for entry in jwalk::WalkDir::new(left) {
        if let Ok(dir_entry) = entry {
            if dir_entry.file_type().is_file() {
                let rel_path = dir_entry.path().strip_prefix(left).unwrap().to_string_lossy().to_string();
                if let Ok(meta) = dir_entry.metadata() {
                    left_files.insert(rel_path, meta);
                }
            }
            left_scan_pb.inc(1);
        }
    }
    left_scan_pb.finish_with_message("Left scan complete");

    let right_scan_pb = ProgressBar::new(right_total as u64);
    right_scan_pb.set_style(ProgressStyle::with_template("[Right {elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {percent}% {msg}").unwrap());
    let mut right_files = FxHashMap::default();
    for entry in jwalk::WalkDir::new(right) {
        if let Ok(dir_entry) = entry {
            if dir_entry.file_type().is_file() {
                let rel_path = dir_entry.path().strip_prefix(right).unwrap().to_string_lossy().to_string();
                if let Ok(meta) = dir_entry.metadata() {
                    right_files.insert(rel_path, meta);
                }
            }
            right_scan_pb.inc(1);
        }
    }
    right_scan_pb.finish_with_message("Right scan complete");
    let phase2_time = phase2_start.elapsed();
    eprintln!("[BENCH] Phase 2 (scanning) duration: {:.2?}", phase2_time);

    // use indicatif::{ProgressBar, ProgressStyle};
    let phase3_start = Instant::now();
    println!("About to start diff calculation...");
    let writer = Arc::new(Mutex::new(writer));
    let all_only_in_left = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let processed_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let total_diffs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let all_paths: FxHashSet<_> = left_files.keys().chain(right_files.keys()).collect();
    let total_files = all_paths.len();
    println!("Processing {} files in parallel...", total_files);
    let chunk_size = 1000;
    let paths_vec: Vec<_> = all_paths.into_iter().collect();
    let pb = ProgressBar::new(total_files as u64);
    pb.set_style(ProgressStyle::with_template("[{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {percent}% ETA:{eta}").unwrap());
    {
        let mut w = writer.lock().unwrap();
        writeln!(w, "Differences:").ok();
    }
    paths_vec.chunks(chunk_size).for_each(|chunk| {
        let writer = Arc::clone(&writer);
        let all_only_in_left = Arc::clone(&all_only_in_left);
        let processed_count = Arc::clone(&processed_count);
        let total_diffs = Arc::clone(&total_diffs);
        let pb = pb.clone();
        rayon::scope(|s| {
            s.spawn(|_| {
                let mut local_buf = Vec::with_capacity(chunk.len());
                for path in chunk {
                    let _count = processed_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    let diff_opt = match (left_files.get(*path), right_files.get(*path)) {
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
                                if left_size < 1024 {
                                    let left_path = left.join(*path);
                                    let right_path = right.join(*path);
                                    if let Some(are_equal) = compare_small_files(&left_path, &right_path) {
                                        if !are_equal {
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
                                } else {
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
                            all_only_in_left.store(false, std::sync::atomic::Ordering::SeqCst);
                            Some(Diff {
                                path: (*path).clone(),
                                diff_type: DiffType::OnlyInRight,
                            })
                        }
                        (None, None) => None,
                    };
                    if let Some(diff) = diff_opt {
                        local_buf.push(format!("Diff: {:?}", diff));
                        total_diffs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                    pb.inc(1);
                }
                if !local_buf.is_empty() {
                    let mut w = writer.lock().unwrap();
                    for line in local_buf {
                        writeln!(w, "{}", line).ok();
                    }
                }
            });
        });
    });
    pb.finish_with_message("Diff calculation and output complete");
    let phase3_time = phase3_start.elapsed();
    eprintln!("[BENCH] Phase 3 (diffing) duration: {:.2?}", phase3_time);
    let total_diffs = total_diffs.load(std::sync::atomic::Ordering::SeqCst);
    {
        let mut w = writer.lock().unwrap();
        writeln!(w, "Total differences found: {}", total_diffs).ok();
        w.flush().ok();
    }
    println!("Output written to {}", output_path.display());

    let total_time = total_start.elapsed();
    eprintln!("[BENCH] Total duration: {:.2?}", total_time);
}

