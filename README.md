# folder-differ

A high-performance, parallel, and robust directory comparison and sync tool written in Rust.

Developed using Windsurf and Cursor.

## Features
- **Parallel directory scanning** with jwalk for both trees
- **Efficient file comparison** using size, modification time, and fast hashing (BLAKE3)
- **Hash sampling for huge files**: only the first and last 64KB are hashed for files >100MB
- **Memory-mapped and parallel hashing** for large files
- **Direct content comparison** for very small files
- **Batch output**: diffs are buffered and written in batches for speed
- **Streaming output**: minimal memory usage, even for millions of diffs
- **Progress bars** for counting, scanning (separate for left/right), and diffing/output (with ETA)
- **Smart diff output**: all diffs streamed to file, summary at end
- **Sync and rollback support** (with backup, if enabled)
- **Synthetic benchmark mode** for performance testing
- **Configurable parallelism**: set thread count via CLI
- **Ignore patterns**: skip files/folders by pattern (basic substring, can be extended)
- **Help/usage output**: `--help` for all options

## Usage

```
folder-differ <left_dir> <right_dir> [--threads N] [--sync] [--dry-run] [--rollback] [--synthetic-benchmark] [--help]
```

### Arguments
- `<left_dir>`: Path to the left directory
- `<right_dir>`: Path to the right directory

### Options
- `--threads N`             : Set number of threads for parallelism (default: 2x logical CPUs)
- `--sync`                  : Plan and perform sync actions (copy/delete files)
- `--dry-run`               : Show planned sync actions without making changes
- `--rollback`              : Roll back the last sync operation using backups
- `--synthetic-benchmark`   : Run a synthetic benchmark (creates and scans a large fake tree)
- `--help`                  : Show help/usage message

## How It Works

1. **Counting Phase**: Recursively counts files and directories in both trees, with a progress bar for each.
2. **Scanning Phase**: Uses jwalk for fast, parallel file listing, with separate progress bars for left and right.
3. **Diff Calculation**: Compares all files by path:
   - If only in left/right: marked as such
   - If sizes differ: marked as different
   - If times differ: hashes compared (BLAKE3, hash sampling for huge files, memory-mapped for large files, direct compare for small)
   - If same size/time: assumed identical
   - Progress bar with ETA during this phase
4. **Diff Output**: 
   - All diffs streamed to output file (buffered, thread-safe)
   - Summary at end
5. **Sync/Backup/Rollback** (if enabled):
   - Plans and performs sync actions (copy, delete, backup)
   - Logs actions and supports rollback using backups

## Example

```
folder-differ /path/to/dirA /path/to/dirB --threads 32
```

## Performance
- Uses jwalk for fast, parallel directory traversal
- Uses BLAKE3 for fast, parallel hashing
- Hash sampling for huge files
- Batch and streaming output for minimal memory usage
- Progress bars for all major phases

## Crates Used
- [`jwalk`](https://crates.io/crates/jwalk) (parallel directory traversal)
- [`blake3`](https://crates.io/crates/blake3) (fast, parallel hashing)
- [`memmap2`](https://crates.io/crates/memmap2) (memory-mapped file access)
- [`indicatif`](https://crates.io/crates/indicatif) (progress bars)
- [`rayon`](https://crates.io/crates/rayon) (parallelism)
- [`rustc-hash`](https://crates.io/crates/rustc-hash) (fast hash maps/sets)
- [`ignore`](https://crates.io/crates/ignore) (ignore pattern support, can be extended)
- [`num_cpus`](https://crates.io/crates/num_cpus) (CPU count for thread pool)

## Requirements
- Rust (edition 2024)

## License
MIT 