mod diff;
mod sync;
mod hash;
mod progress;

use std::fs::{self, File, Metadata};
use std::path::Path;
use std::io::{BufWriter, Write};
use std::time::Instant;
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicUsize;
use indicatif::{ProgressBar, ProgressStyle, ProgressDrawTarget};
use folder_differ::get_dir_files_with_ignore;

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
        progress::run_synthetic_benchmark();
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

    // PHASE 1: Count files and directories (with progress bar)
    let scan_start = Instant::now();
    let count_pb = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr());
    count_pb.set_style(ProgressStyle::with_template("[Counting {elapsed_precise}] {msg}").unwrap());
    use std::sync::atomic::{AtomicUsize, Ordering};
    let left_file_count = Arc::new(AtomicUsize::new(0));
    let left_dir_count = Arc::new(AtomicUsize::new(0));
    let right_file_count = Arc::new(AtomicUsize::new(0));
    let right_dir_count = Arc::new(AtomicUsize::new(0));
    let left_active_tasks = Arc::new(AtomicUsize::new(1));
    let left_max_tasks = Arc::new(AtomicUsize::new(1));
    let right_active_tasks = Arc::new(AtomicUsize::new(1));
    let right_max_tasks = Arc::new(AtomicUsize::new(1));
    rayon::join(
        || progress::count_files_dirs(left, &left_file_count, &left_dir_count, &count_pb, &left_active_tasks, &left_max_tasks),
        || progress::count_files_dirs(right, &right_file_count, &right_dir_count, &count_pb, &right_active_tasks, &right_max_tasks),
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

    // PHASE 3: Diff calculation and output
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
                                Some(diff::Diff {
                                    path: (*path).clone(),
                                    diff_type: diff::DiffType::Different { left_size, right_size, left_time, right_time },
                                })
                            } else if left_time != right_time {
                                if left_size < 1024 {
                                    let left_path = left.join(*path);
                                    let right_path = right.join(*path);
                                    if let Some(are_equal) = hash::compare_small_files(&left_path, &right_path) {
                                        if !are_equal {
                                            Some(diff::Diff {
                                                path: (*path).clone(),
                                                diff_type: diff::DiffType::Different { left_size, right_size, left_time, right_time },
                                            })
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                } else {
                                    let left_hash = hash::hash_file(&left.join(*path));
                                    let right_hash = hash::hash_file(&right.join(*path));
                                    if left_hash != right_hash {
                                        Some(diff::Diff {
                                            path: (*path).clone(),
                                            diff_type: diff::DiffType::Different { left_size, right_size, left_time, right_time },
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
                            Some(diff::Diff {
                                path: (*path).clone(),
                                diff_type: diff::DiffType::OnlyInLeft,
                            })
                        }
                        (None, Some(_)) => {
                            all_only_in_left.store(false, std::sync::atomic::Ordering::SeqCst);
                            Some(diff::Diff {
                                path: (*path).clone(),
                                diff_type: diff::DiffType::OnlyInRight,
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

