//! Progress bar and benchmarking utilities for folder-differ

use std::path::Path;
use std::sync::atomic::AtomicUsize;
use indicatif::ProgressBar;

/// Run a synthetic benchmark by creating and scanning a large fake tree.
pub fn run_synthetic_benchmark() {
    use std::time::Instant;
    use std::fs;
    use std::sync::Arc;
    let root = Path::new("/tmp/folder-differ-bench");
    let n_dirs = 100;
    let n_files_per_dir = 100;
    let file_size = 4096;
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
        file_count.load(std::sync::atomic::Ordering::SeqCst),
        dir_count.load(std::sync::atomic::Ordering::SeqCst),
        elapsed,
        max_tasks.load(std::sync::atomic::Ordering::SeqCst));
    let _ = fs::remove_dir_all(root);
    println!("Synthetic benchmark finished and cleaned up.");
}

/// Recursively count files and directories, updating progress bar.
pub fn count_files_dirs(
    root: &Path,
    file_count: &std::sync::Arc<AtomicUsize>,
    dir_count: &std::sync::Arc<AtomicUsize>,
    pb: &indicatif::ProgressBar,
    active_tasks: &std::sync::Arc<std::sync::atomic::AtomicUsize>,
    max_tasks: &std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use indicatif::ProgressBar;

    #[test]
    fn test_count_files_dirs_simple() {
        let dir = tempdir().unwrap();
        let subdir = dir.path().join("sub");
        fs::create_dir(&subdir).unwrap();
        fs::write(dir.path().join("a.txt"), b"foo").unwrap();
        fs::write(subdir.join("b.txt"), b"bar").unwrap();
        let file_count = Arc::new(AtomicUsize::new(0));
        let dir_count = Arc::new(AtomicUsize::new(0));
        let active_tasks = Arc::new(AtomicUsize::new(1));
        let max_tasks = Arc::new(AtomicUsize::new(1));
        let pb = ProgressBar::hidden();
        count_files_dirs(
            dir.path(),
            &file_count,
            &dir_count,
            &pb,
            &active_tasks,
            &max_tasks,
        );
        assert_eq!(file_count.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(dir_count.load(std::sync::atomic::Ordering::SeqCst), 1); // only the subdir is counted
    }
} 