use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const ARTIFACT_RECORD_SCHEMA: &str = "forge.artifact.v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactClass {
    PatchPending,
    PatchApplied,
    PatchFailed,
    PatchQuarantine,
    Debug,
    Diagnostics,
    Build,
    Release,
    Report,
    SourceControl,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactEvidence {
    pub schema: String,
    pub id: String,
    pub project_id: String,
    pub class: ArtifactClass,
    pub source: PathBuf,
    pub stored: PathBuf,
    pub bytes: u64,
    pub sha256: String,
    pub operation_id: Option<String>,
    pub created_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPlan {
    pub project_id: String,
    pub class: ArtifactClass,
    pub keep_latest: usize,
    pub keep: Vec<PathBuf>,
    pub archive: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ArtifactCentral {
    root: PathBuf,
}

impl ArtifactCentral {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, String> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root).map_err(|error| format!("failed to create Artifact Central {}: {error}", root.display()))?;
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path { &self.root }

    pub fn project_root(&self, project_id: &str) -> Result<PathBuf, String> {
        validate_id(project_id)?;
        Ok(self.root.join("projects").join(project_id))
    }

    pub fn ensure_project_layout(&self, project_id: &str) -> Result<PathBuf, String> {
        let root = self.project_root(project_id)?;
        for rel in [
            "patches/pending", "patches/applied", "patches/failed", "patches/quarantine",
            "debug", "diagnostics", "builds", "releases", "reports", "source-control",
            "other", "index", "archive",
        ] {
            fs::create_dir_all(root.join(rel)).map_err(|error| error.to_string())?;
        }
        Ok(root)
    }

    pub fn promote(
        &self,
        project_id: &str,
        class: ArtifactClass,
        source: &Path,
        operation_id: Option<String>,
    ) -> Result<ArtifactEvidence, String> {
        if !source.is_file() {
            return Err(format!("artifact source is not a file: {}", source.display()));
        }
        let project_root = self.ensure_project_layout(project_id)?;
        let class_root = project_root.join(class_relative(class));
        fs::create_dir_all(&class_root).map_err(|error| error.to_string())?;
        let file_name = source.file_name().ok_or_else(|| "artifact source has no file name".to_owned())?;
        let destination = unique_destination(&class_root, file_name);
        let temp = destination.with_extension(format!("forge-copy-{}", unix_ms()));
        fs::copy(source, &temp).map_err(|error| error.to_string())?;
        let source_hash = sha256_file(source)?;
        let temp_hash = sha256_file(&temp)?;
        if source_hash != temp_hash {
            let _ = fs::remove_file(&temp);
            return Err("Artifact Central copy verification failed".to_owned());
        }
        fs::rename(&temp, &destination).map_err(|error| error.to_string())?;
        let metadata = fs::metadata(&destination).map_err(|error| error.to_string())?;
        let evidence = ArtifactEvidence {
            schema: ARTIFACT_RECORD_SCHEMA.to_owned(),
            id: format!("artifact-{}", unix_ms()),
            project_id: project_id.to_owned(),
            class,
            source: source.to_path_buf(),
            stored: destination,
            bytes: metadata.len(),
            sha256: source_hash,
            operation_id,
            created_unix_ms: unix_ms(),
        };
        self.write_evidence(&evidence)?;
        Ok(evidence)
    }

    pub fn retention_plan(
        &self,
        project_id: &str,
        class: ArtifactClass,
        keep_latest: usize,
    ) -> Result<RetentionPlan, String> {
        let root = self.ensure_project_layout(project_id)?.join(class_relative(class));
        let mut entries = fs::read_dir(&root)
            .map_err(|error| error.to_string())?
            .flatten()
            .filter(|entry| entry.path().is_file())
            .map(|entry| {
                let modified = entry.metadata().ok().and_then(|metadata| metadata.modified().ok()).unwrap_or(UNIX_EPOCH);
                (modified, entry.path())
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        let keep = entries.iter().take(keep_latest).map(|(_, path)| path.clone()).collect();
        let archive = entries.iter().skip(keep_latest).map(|(_, path)| path.clone()).collect();
        Ok(RetentionPlan { project_id: project_id.to_owned(), class, keep_latest, keep, archive })
    }

    pub fn apply_retention(&self, plan: &RetentionPlan) -> Result<usize, String> {
        let project_root = self.ensure_project_layout(&plan.project_id)?;
        let archive_root = project_root.join("archive").join(class_relative(plan.class));
        fs::create_dir_all(&archive_root).map_err(|error| error.to_string())?;
        let mut moved = 0usize;
        for path in &plan.archive {
            if !path.is_file() { continue; }
            let name = path.file_name().ok_or_else(|| "retention file has no name".to_owned())?;
            let destination = unique_destination(&archive_root, name);
            fs::rename(path, destination).map_err(|error| error.to_string())?;
            moved += 1;
        }
        Ok(moved)
    }

    fn write_evidence(&self, evidence: &ArtifactEvidence) -> Result<(), String> {
        let root = self.project_root(&evidence.project_id)?.join("index");
        fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let path = root.join(format!("{}.json", evidence.id));
        let bytes = serde_json::to_vec_pretty(evidence).map_err(|error| error.to_string())?;
        atomic_write(&path, &bytes)
    }
}

fn class_relative(class: ArtifactClass) -> &'static str {
    match class {
        ArtifactClass::PatchPending => "patches/pending",
        ArtifactClass::PatchApplied => "patches/applied",
        ArtifactClass::PatchFailed => "patches/failed",
        ArtifactClass::PatchQuarantine => "patches/quarantine",
        ArtifactClass::Debug => "debug",
        ArtifactClass::Diagnostics => "diagnostics",
        ArtifactClass::Build => "builds",
        ArtifactClass::Release => "releases",
        ArtifactClass::Report => "reports",
        ArtifactClass::SourceControl => "source-control",
        ArtifactClass::Other => "other",
    }
}

fn validate_id(value: &str) -> Result<(), String> {
    if value.len() < 2 || !value.chars().all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')) {
        return Err(format!("invalid Artifact Central project id: {value}"));
    }
    Ok(())
}

fn unique_destination(root: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let base = root.join(name);
    if !base.exists() { return base; }
    let name = name.to_string_lossy();
    root.join(format!("{}-{}", unix_ms(), name))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 { break; }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    Ok(out)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "artifact index path has no parent".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temp = parent.join(format!(".forge-artifact-{}.tmp", unix_ms()));
    let mut file = File::create(&temp).map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.flush().map_err(|error| error.to_string())?;
    fs::rename(temp, path).map_err(|error| error.to_string())
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_paths_are_project_scoped() {
        assert_eq!(class_relative(ArtifactClass::PatchPending), "patches/pending");
        assert!(validate_id("cortex").is_ok());
        assert!(validate_id("../bad").is_err());
    }
}
