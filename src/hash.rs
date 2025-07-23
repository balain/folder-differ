//! File hashing logic for folder-differ

use std::fs::File;
use std::io::{Read, BufReader, Seek};
use std::path::Path;
use memmap2::Mmap;
use blake3;

/// Hash a file, using sampling for large files.
pub fn hash_file(path: &Path) -> Option<Vec<u8>> {
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

/// Hash only the first and last 64KB of a large file (>100MB).
pub fn hash_sampled_file(path: &Path) -> Option<Vec<u8>> {
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

/// Hash a small file (<1KB).
pub fn hash_small_file(path: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let mut content = Vec::new();
    file.read_to_end(&mut content).ok()?;
    let hash = blake3::hash(&content);
    Some(hash.as_bytes().to_vec())
}

/// Hash a medium-sized file (1KB-1MB) using BLAKE3.
pub fn hash_medium_file_blake3(path: &Path) -> Option<Vec<u8>> {
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

/// Hash a large file (>1MB) using BLAKE3 and memory mapping.
pub fn hash_large_file_blake3(path: &Path) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    let mmap = unsafe { Mmap::map(&file).ok()? };
    let hash = blake3::Hasher::new().update(&mmap).finalize();
    Some(hash.as_bytes().to_vec())
}

/// Compare two small files for byte equality.
pub fn compare_small_files(left_path: &Path, right_path: &Path) -> Option<bool> {
    let mut left_content = Vec::new();
    let mut right_content = Vec::new();
    File::open(left_path).ok()?.read_to_end(&mut left_content).ok()?;
    File::open(right_path).ok()?.read_to_end(&mut right_content).ok()?;
    Some(left_content == right_content)
} 