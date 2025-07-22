# folder-differ

A fast, parallel, and robust directory comparison and sync tool written in Rust.

Developed using Windsurf and Cursor.

## Features
- **Parallel directory scanning** with progress bars
- **Efficient file comparison** using size, modification time, and fast hashing (xxHash3)
- **Memory-mapped hashing** for large files
- **Direct content comparison** for very small files
- **Chunked parallel diffing** for stability and speed
- **Progress reporting** during all major phases
- **Smart diff output**: prints all diffs if <200 or all are only in left, otherwise shows first 10
- **Sync and rollback support** (with backup)
- **Synthetic benchmark mode** for performance testing

## Usage

```
folder-differ <left_dir> <right_dir> [--sync] [--dry-run] [--rollback] [--synthetic-benchmark]
```

### Arguments
- `<left_dir>`: Path to the left directory
- `<right_dir>`: Path to the right directory

### Options
- `--sync`                : Plan and perform sync actions (copy/delete files)
- `--dry-run`             : Show planned sync actions without making changes
- `--rollback`            : Roll back the last sync operation using backups
- `--synthetic-benchmark` : Run a synthetic benchmark (creates and scans a large fake tree)

## How It Works

1. **Counting Phase**: Recursively counts files and directories in both trees, with a progress bar.
2. **Scanning Phase**: Scans both trees, collecting file metadata, with a percent-complete progress bar.
3. **Diff Calculation**: Compares all files by path:
   - If only in left/right: marked as such
   - If sizes differ: marked as different
   - If times differ: hashes compared (xxHash3, memory-mapped for large files, direct compare for small)
   - If same size/time: assumed identical
   - Progress is shown during this phase
4. **Diff Output**: 
   - If <200 diffs or all are only in left: prints all
   - Otherwise: prints first 10 and a summary
5. **Sync/Backup/Rollback** (if enabled):
   - Plans and performs sync actions (copy, delete, backup)
   - Logs actions and supports rollback using backups

## Example

```
folder-differ /path/to/dirA /path/to/dirB
```

## Performance
- Uses parallelism for scanning and diffing
- Uses xxHash3 for fast hashing
- Uses memory mapping for large files
- Skips unnecessary hashing when possible

## Safety
- Sync actions create backups before overwriting or deleting
- Rollback restores from backups if needed

## Benchmarking
Run with `--synthetic-benchmark` to test performance on a large, fake directory tree.

## Requirements
- Rust (edition 2024)

## License
MIT 