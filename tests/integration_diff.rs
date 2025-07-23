use folder_differ::{diff::compare_dirs, get_dir_files_with_ignore};
use std::fs::File;
use std::io::Write;
use tempfile::tempdir;

fn write_file(path: &std::path::Path, content: &[u8]) {
    let mut file = File::create(path).unwrap();
    file.write_all(content).unwrap();
}

#[test]
fn integration_identical_dirs() {
    let dir1 = tempdir().unwrap();
    let dir2 = tempdir().unwrap();
    write_file(&dir1.path().join("a.txt"), b"hello");
    write_file(&dir2.path().join("a.txt"), b"hello");
    let diffs = compare_dirs(dir1.path(), dir2.path()).unwrap();
    assert!(diffs.is_empty(), "No diffs expected for identical dirs");
}

#[test]
fn integration_file_only_in_left() {
    let dir1 = tempdir().unwrap();
    let dir2 = tempdir().unwrap();
    write_file(&dir1.path().join("a.txt"), b"hello");
    let diffs = compare_dirs(dir1.path(), dir2.path()).unwrap();
    assert_eq!(diffs.len(), 1);
    assert!(matches!(
        diffs[0].diff_type,
        folder_differ::diff::DiffType::OnlyInLeft
    ));
}
