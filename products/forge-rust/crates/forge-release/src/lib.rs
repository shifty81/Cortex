use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseManifest {
    pub schema: String,
    pub version: String,
    pub created_unix_ms: u64,
    pub files: Vec<ReleaseFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryCheckpoint {
    pub schema: String,
    pub id: String,
    pub root: PathBuf,
    pub files: Vec<ReleaseFile>,
    pub created_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdatePlan {
    pub schema: String,
    pub from_version: String,
    pub to_version: String,
    pub add_or_replace: Vec<ReleaseFile>,
    pub remove: Vec<String>,
    pub changed_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestoreReceipt {
    pub schema: String,
    pub checkpoint_id: String,
    pub restored_files: Vec<String>,
    pub unix_ms: u64,
}

pub fn build_manifest(
    root: &Path,
    version: &str,
    max_files: usize,
) -> Result<ReleaseManifest, String> {
    let mut files = Vec::new();
    collect(root, root, max_files, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(ReleaseManifest {
        schema: "forge.release.v1".to_owned(),
        version: version.to_owned(),
        created_unix_ms: unix_ms(),
        files,
    })
}

pub fn verify_manifest(root: &Path, manifest: &ReleaseManifest) -> Result<(), String> {
    for entry in &manifest.files {
        let path = root.join(entry.path.split('/').collect::<PathBuf>());
        if !path.is_file() {
            return Err(format!("release file missing: {}", entry.path));
        }
        let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
        if metadata.len() != entry.bytes {
            return Err(format!("release byte mismatch: {}", entry.path));
        }
        if sha256_file(&path)? != entry.sha256 {
            return Err(format!("release hash mismatch: {}", entry.path));
        }
    }
    Ok(())
}

pub fn plan_update(current: &ReleaseManifest, target: &ReleaseManifest) -> UpdatePlan {
    let current_by_path = current
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<std::collections::BTreeMap<_, _>>();
    let target_by_path = target
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut add_or_replace = Vec::new();
    let mut remove = Vec::new();
    for file in &target.files {
        if current_by_path
            .get(file.path.as_str())
            .is_none_or(|current| current.sha256 != file.sha256 || current.bytes != file.bytes)
        {
            add_or_replace.push(file.clone());
        }
    }
    for file in &current.files {
        if !target_by_path.contains_key(file.path.as_str()) {
            remove.push(file.path.clone());
        }
    }
    UpdatePlan {
        schema: "forge.update_plan.v1".to_owned(),
        from_version: current.version.clone(),
        to_version: target.version.clone(),
        changed_files: add_or_replace.len() + remove.len(),
        add_or_replace,
        remove,
    }
}

pub fn create_recovery_checkpoint(
    source_root: &Path,
    recovery_root: &Path,
    relative_files: &[String],
) -> Result<RecoveryCheckpoint, String> {
    let id = format!("recovery-{}", unix_ms());
    let root = recovery_root.join(&id);
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let mut files = Vec::new();
    for relative in relative_files {
        validate_relative(relative)?;
        let source = source_root.join(relative.split('/').collect::<PathBuf>());
        if !source.is_file() {
            continue;
        }
        let destination = root.join(relative.split('/').collect::<PathBuf>());
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::copy(&source, &destination).map_err(|error| error.to_string())?;
        let metadata = fs::metadata(&destination).map_err(|error| error.to_string())?;
        files.push(ReleaseFile {
            path: relative.clone(),
            bytes: metadata.len(),
            sha256: sha256_file(&destination)?,
        });
    }
    let checkpoint = RecoveryCheckpoint {
        schema: "forge.recovery.checkpoint.v1".to_owned(),
        id,
        root: root.clone(),
        files,
        created_unix_ms: unix_ms(),
    };
    let bytes = serde_json::to_vec_pretty(&checkpoint).map_err(|error| error.to_string())?;
    let mut file = File::create(root.join("checkpoint.json")).map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    Ok(checkpoint)
}

pub fn restore_checkpoint(
    destination_root: &Path,
    checkpoint: &RecoveryCheckpoint,
    approve: bool,
) -> Result<RestoreReceipt, String> {
    if !approve {
        return Err("recovery restore requires explicit approval".to_owned());
    }
    let mut restored_files = Vec::new();
    for file in &checkpoint.files {
        validate_relative(&file.path)?;
        let source = checkpoint.root.join(file.path.split('/').collect::<PathBuf>());
        if !source.is_file() {
            return Err(format!("recovery source missing: {}", file.path));
        }
        if sha256_file(&source)? != file.sha256 {
            return Err(format!("recovery source hash mismatch: {}", file.path));
        }
        let destination = destination_root.join(file.path.split('/').collect::<PathBuf>());
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let temp = destination.with_extension(format!("forge-restore-{}.tmp", unix_ms()));
        fs::copy(&source, &temp).map_err(|error| error.to_string())?;
        if destination.exists() {
            fs::remove_file(&destination).map_err(|error| error.to_string())?;
        }
        fs::rename(&temp, &destination).map_err(|error| error.to_string())?;
        restored_files.push(file.path.clone());
    }
    Ok(RestoreReceipt {
        schema: "forge.restore_receipt.v1".to_owned(),
        checkpoint_id: checkpoint.id.clone(),
        restored_files,
        unix_ms: unix_ms(),
    })
}

fn collect(
    root: &Path,
    current: &Path,
    max_files: usize,
    out: &mut Vec<ReleaseFile>,
) -> Result<(), String> {
    if out.len() >= max_files {
        return Err("release manifest file budget exceeded".to_owned());
    }
    for entry in fs::read_dir(current)
        .map_err(|error| error.to_string())?
        .flatten()
    {
        let path = entry.path();
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect(root, &path, max_files, out)?;
        } else if kind.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            let metadata = entry.metadata().map_err(|error| error.to_string())?;
            out.push(ReleaseFile {
                path: relative,
                bytes: metadata.len(),
                sha256: sha256_file(&path)?,
            });
            if out.len() >= max_files {
                return Err("release manifest file budget exceeded".to_owned());
            }
        }
    }
    Ok(())
}

fn validate_relative(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.trim().is_empty()
        || path.is_absolute()
        || value.split('/').any(|part| part == ".." || part.is_empty())
    {
        Err(format!("unsafe recovery path: {value}"))
    } else {
        Ok(())
    }
}

fn sha256_file(path: &Path) -> Result<String, String> {
    use std::fmt::Write as _;
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(&mut out, "{byte:02x}");
    }
    Ok(out)
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
    fn blocks_recovery_traversal() {
        assert!(validate_relative("src/lib.rs").is_ok());
        assert!(validate_relative("../x").is_err());
    }

    #[test]
    fn update_plan_detects_replace_and_remove() {
        let current = ReleaseManifest {
            schema: "forge.release.v1".to_owned(),
            version: "1".to_owned(),
            created_unix_ms: 0,
            files: vec![
                ReleaseFile { path: "a".to_owned(), bytes: 1, sha256: "x".to_owned() },
                ReleaseFile { path: "old".to_owned(), bytes: 1, sha256: "o".to_owned() },
            ],
        };
        let target = ReleaseManifest {
            schema: "forge.release.v1".to_owned(),
            version: "2".to_owned(),
            created_unix_ms: 0,
            files: vec![ReleaseFile { path: "a".to_owned(), bytes: 2, sha256: "y".to_owned() }],
        };
        let plan = plan_update(&current, &target);
        assert_eq!(plan.add_or_replace.len(), 1);
        assert_eq!(plan.remove, vec!["old"]);
    }
}
