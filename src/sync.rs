//! Synchronization actions, logging, and rollback for folder-differ

use crate::Result;
use crate::diff::{Diff, DiffType};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Represents an action to synchronize files between directories.
#[derive(Debug, Clone)]
pub enum SyncAction {
    CopyLeftToRight(String),
    CopyRightToLeft(String),
    DeleteLeft(String),
    DeleteRight(String),
    Conflict(String),
    NoOp(String),
}

/// A log entry for a sync action.
#[derive(Debug, Clone)]
pub struct SyncLogEntry {
    pub action: SyncAction,
    pub timestamp: SystemTime,
    pub details: String,
}

/// A log of all sync actions performed.
#[derive(Debug, Clone, Default)]
pub struct SyncLog {
    pub entries: Vec<SyncLogEntry>,
}

/// State information for synchronization.
#[derive(Debug, Clone, Default)]
pub struct SyncState {
    pub last_synced: Option<SystemTime>,
}

/// Plan sync actions based on diffs and sync mode.
pub fn plan_sync_actions(diffs: &[Diff], _sync_mode: &str) -> Vec<SyncAction> {
    diffs
        .iter()
        .map(|diff| match &diff.diff_type {
            DiffType::OnlyInLeft => SyncAction::CopyLeftToRight(diff.path.clone()),
            DiffType::OnlyInRight => SyncAction::CopyRightToLeft(diff.path.clone()),
            DiffType::Different { .. } => SyncAction::CopyLeftToRight(diff.path.clone()),
        })
        .collect()
}

/// Log a sync action.
pub fn log_sync_action(log: &mut SyncLog, action: &SyncAction, details: &str) {
    log.entries.push(SyncLogEntry {
        action: action.clone(),
        timestamp: SystemTime::now(),
        details: details.to_string(),
    });
}

/// Save the sync log to disk.
pub fn save_sync_log(log: &SyncLog, path: &Path) -> Result<()> {
    let log_path = path.join(".sync-log.txt");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    for entry in &log.entries {
        writeln!(
            file,
            "{:?} at {:?}: {}",
            entry.action, entry.timestamp, entry.details
        )?;
    }
    Ok(())
}

/// Save the sync state to disk.
pub fn save_sync_state(_state: &SyncState, path: &Path) -> Result<()> {
    std::fs::write(path.join(".sync-state.json"), "{}\n")?;
    Ok(())
}

/// Create a backup of a file before modification.
pub fn backup_file(path: &Path) -> Result<Option<PathBuf>> {
    if path.exists() {
        let backup_path = path.with_extension("bak");
        std::fs::copy(path, &backup_path)?;
        Ok(Some(backup_path))
    } else {
        Ok(None)
    }
}

/// Restore a file from its backup.
pub fn restore_file(backup_path: &Path, orig_path: &Path) -> Result<()> {
    std::fs::copy(backup_path, orig_path)?;
    Ok(())
}

/// Delete a file with backup.
pub fn delete_file_with_backup(path: &Path) -> Result<Option<PathBuf>> {
    let backup = backup_file(path)?;
    std::fs::remove_file(path)?;
    Ok(backup)
}

/// Perform a sync action.
pub fn perform_sync_action(
    action: &SyncAction,
    left: &Path,
    right: &Path,
    log: &mut SyncLog,
) -> Result<()> {
    match action {
        SyncAction::CopyLeftToRight(rel_path) => {
            let src = left.join(rel_path);
            let dst = right.join(rel_path);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let backup = backup_file(&dst)?;
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
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let backup = backup_file(&dst)?;
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
            let backup = delete_file_with_backup(&path)?;
            let msg = if backup.is_some() {
                format!("Deleted {} from left. Backup: {:?}", rel_path, backup)
            } else {
                format!("FAILED to delete {} from left", rel_path)
            };
            log_sync_action(log, action, &msg);
        }
        SyncAction::DeleteRight(rel_path) => {
            let path = right.join(rel_path);
            let backup = delete_file_with_backup(&path)?;
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
    Ok(())
}

/// Roll back all sync actions in the log.
pub fn rollback(log: &SyncLog, left: &Path, right: &Path) -> Result<()> {
    for entry in log.entries.iter().rev() {
        match &entry.action {
            SyncAction::CopyLeftToRight(rel_path) => {
                let dst = right.join(rel_path);
                let backup = dst.with_extension("bak");
                if backup.exists() {
                    restore_file(&backup, &dst)?;
                    std::fs::remove_file(&backup)?;
                } else {
                    let _ = std::fs::remove_file(&dst);
                }
                println!("Rolled back CopyLeftToRight: {}", rel_path);
            }
            SyncAction::CopyRightToLeft(rel_path) => {
                let dst = left.join(rel_path);
                let backup = dst.with_extension("bak");
                if backup.exists() {
                    restore_file(&backup, &dst)?;
                    std::fs::remove_file(&backup)?;
                } else {
                    let _ = std::fs::remove_file(&dst);
                }
                println!("Rolled back CopyRightToLeft: {}", rel_path);
            }
            SyncAction::DeleteLeft(rel_path) => {
                let orig = left.join(rel_path);
                let backup = orig.with_extension("bak");
                if backup.exists() {
                    restore_file(&backup, &orig)?;
                    std::fs::remove_file(&backup)?;
                }
                println!("Rolled back DeleteLeft: {}", rel_path);
            }
            SyncAction::DeleteRight(rel_path) => {
                let orig = right.join(rel_path);
                let backup = orig.with_extension("bak");
                if backup.exists() {
                    restore_file(&backup, &orig)?;
                    std::fs::remove_file(&backup)?;
                }
                println!("Rolled back DeleteRight: {}", rel_path);
            }
            SyncAction::Conflict(rel_path) | SyncAction::NoOp(rel_path) => {
                println!("No rollback for action on {}", rel_path);
            }
        }
    }
    Ok(())
}
