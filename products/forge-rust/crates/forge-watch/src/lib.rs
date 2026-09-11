use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const WATCH_SCHEMA: &str = "forge.watch.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatchConfig {
    pub roots: Vec<PathBuf>,
    pub recursive: bool,
    pub max_entries: usize,
    pub ignored_directory_names: BTreeSet<String>,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            recursive: true,
            max_entries: 250_000,
            ignored_directory_names: [
                ".git",
                ".cortex",
                ".project_control",
                "target",
                "node_modules",
                "artifacts",
                "build",
                "dist",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileStamp {
    pub path: PathBuf,
    pub size: u64,
    pub modified_unix_ms: u64,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatchSnapshot {
    pub schema: String,
    pub observed_unix_ms: u64,
    pub truncated: bool,
    pub entries: BTreeMap<String, FileStamp>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WatchChangeKind {
    Created,
    Modified,
    Removed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatchChange {
    pub kind: WatchChangeKind,
    pub path: PathBuf,
    pub previous: Option<FileStamp>,
    pub current: Option<FileStamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatchDiff {
    pub schema: String,
    pub previous_unix_ms: u64,
    pub current_unix_ms: u64,
    pub changes: Vec<WatchChange>,
    pub truncated: bool,
}

pub fn capture(config: &WatchConfig) -> Result<WatchSnapshot, String> {
    let mut entries = BTreeMap::new();
    let mut truncated = false;
    for root in &config.roots {
        if entries.len() >= config.max_entries {
            truncated = true;
            break;
        }
        capture_path(root, config, &mut entries, &mut truncated)?;
    }
    Ok(WatchSnapshot {
        schema: WATCH_SCHEMA.to_owned(),
        observed_unix_ms: unix_ms(),
        truncated,
        entries,
    })
}

pub fn diff(previous: &WatchSnapshot, current: &WatchSnapshot) -> WatchDiff {
    let mut changes = Vec::new();
    for (key, now) in &current.entries {
        match previous.entries.get(key) {
            None => changes.push(WatchChange {
                kind: WatchChangeKind::Created,
                path: now.path.clone(),
                previous: None,
                current: Some(now.clone()),
            }),
            Some(before) if before != now => changes.push(WatchChange {
                kind: WatchChangeKind::Modified,
                path: now.path.clone(),
                previous: Some(before.clone()),
                current: Some(now.clone()),
            }),
            Some(_) => {}
        }
    }
    for (key, before) in &previous.entries {
        if !current.entries.contains_key(key) {
            changes.push(WatchChange {
                kind: WatchChangeKind::Removed,
                path: before.path.clone(),
                previous: Some(before.clone()),
                current: None,
            });
        }
    }
    changes.sort_by(|left, right| left.path.cmp(&right.path));
    WatchDiff {
        schema: "forge.watch_diff.v1".to_owned(),
        previous_unix_ms: previous.observed_unix_ms,
        current_unix_ms: current.observed_unix_ms,
        changes,
        truncated: previous.truncated || current.truncated,
    }
}

pub fn save_snapshot(path: &Path, snapshot: &WatchSnapshot) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("watch snapshot has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(snapshot).map_err(|error| error.to_string())?;
    let temp = parent.join(format!(".watch-{}.tmp", unix_ms()));
    fs::write(&temp, bytes).map_err(|error| error.to_string())?;
    if path.exists() {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    fs::rename(&temp, path).map_err(|error| error.to_string())
}

pub fn load_snapshot(path: &Path) -> Result<Option<WatchSnapshot>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn capture_path(
    root: &Path,
    config: &WatchConfig,
    entries: &mut BTreeMap<String, FileStamp>,
    truncated: &mut bool,
) -> Result<(), String> {
    if entries.len() >= config.max_entries {
        *truncated = true;
        return Ok(());
    }
    let metadata = match fs::metadata(root) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(()),
    };
    let stamp = FileStamp {
        path: root.to_path_buf(),
        size: metadata.len(),
        modified_unix_ms: metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or_default(),
        is_dir: metadata.is_dir(),
    };
    entries.insert(path_key(root), stamp);
    if !metadata.is_dir() || !config.recursive {
        return Ok(());
    }
    if ignored(root, config) {
        return Ok(());
    }
    let directory = match fs::read_dir(root) {
        Ok(directory) => directory,
        Err(_) => return Ok(()),
    };
    for entry in directory.flatten() {
        capture_path(&entry.path(), config, entries, truncated)?;
        if entries.len() >= config.max_entries {
            *truncated = true;
            break;
        }
    }
    Ok(())
}

fn ignored(path: &Path, config: &WatchConfig) -> bool {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    config.ignored_directory_names.contains(name)
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_ascii_lowercase()
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_reports_create_modify_remove() {
        let before = WatchSnapshot {
            schema: WATCH_SCHEMA.to_owned(),
            observed_unix_ms: 1,
            truncated: false,
            entries: [
                (
                    "a".to_owned(),
                    FileStamp { path: PathBuf::from("a"), size: 1, modified_unix_ms: 1, is_dir: false },
                ),
                (
                    "b".to_owned(),
                    FileStamp { path: PathBuf::from("b"), size: 1, modified_unix_ms: 1, is_dir: false },
                ),
            ]
            .into_iter()
            .collect(),
        };
        let after = WatchSnapshot {
            schema: WATCH_SCHEMA.to_owned(),
            observed_unix_ms: 2,
            truncated: false,
            entries: [
                (
                    "b".to_owned(),
                    FileStamp { path: PathBuf::from("b"), size: 2, modified_unix_ms: 2, is_dir: false },
                ),
                (
                    "c".to_owned(),
                    FileStamp { path: PathBuf::from("c"), size: 1, modified_unix_ms: 2, is_dir: false },
                ),
            ]
            .into_iter()
            .collect(),
        };
        let result = diff(&before, &after);
        assert_eq!(result.changes.len(), 3);
    }
}
