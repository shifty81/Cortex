use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use zip::ZipArchive;

pub const PATCH_SCHEMA: &str = "forge.patch.v1";
pub const PATCH_ENGINE: &str = "forge-universal";
pub const RECEIPT_SCHEMA: &str = "forge.patch.receipt.v1";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PatchTarget {
    #[serde(rename = "projectId")]
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PatchPreconditions {
    #[serde(rename = "gitCommit")]
    pub git_commit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PatchRequirements {
    #[serde(rename = "projectSchema")]
    pub project_schema: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PatchOperation {
    Add,
    Replace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchFile {
    pub path: String,
    pub operation: PatchOperation,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PatchManifest {
    pub schema: String,
    pub engine: String,
    #[serde(rename = "createdUtc")]
    pub created_utc: String,
    pub project: String,
    pub target: PatchTarget,
    #[serde(rename = "patchId")]
    pub patch_id: String,
    pub title: String,
    pub series: String,
    pub sequence: String,
    pub preconditions: PatchPreconditions,
    pub requires: PatchRequirements,
    #[serde(rename = "minimumGate")]
    pub minimum_gate: String,
    pub files: Vec<PatchFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchValidation {
    pub patch_id: String,
    pub project_id: String,
    pub transport_sha256: String,
    pub manifest_sha256: String,
    pub file_count: usize,
    pub total_payload_bytes: u64,
    pub base_commit: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PatchReceiptStatus {
    Validated,
    Applied,
    RolledBack,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchReceipt {
    pub schema: String,
    pub patch_id: String,
    pub project_id: String,
    pub status: PatchReceiptStatus,
    pub transport_sha256: String,
    pub transaction_root: PathBuf,
    pub changed_paths: Vec<String>,
    pub message: String,
    pub unix_ms: u64,
}

#[derive(Debug, Clone)]
pub struct PatchPackage {
    path: PathBuf,
    manifest: PatchManifest,
    validation: PatchValidation,
}

impl PatchPackage {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        if path.extension().and_then(|value| value.to_str()) != Some("patch") {
            return Err(format!("Forge patch transport must use .patch: {}", path.display()));
        }
        if !path.is_file() {
            return Err(format!("Forge patch does not exist: {}", path.display()));
        }
        let transport_sha256 = sha256_file(&path)?;
        verify_sidecar_if_present(&path, &transport_sha256)?;

        let file = File::open(&path)
            .map_err(|error| format!("failed to open Forge patch {}: {error}", path.display()))?;
        let mut archive = ZipArchive::new(file)
            .map_err(|error| format!("invalid Forge patch ZIP transport {}: {error}", path.display()))?;
        let names = regular_names(&mut archive)?;
        if !names.contains("PATCH_MANIFEST.json") {
            return Err("Forge patch is missing top-level PATCH_MANIFEST.json".to_owned());
        }
        let manifest_bytes = read_member(&mut archive, "PATCH_MANIFEST.json")?;
        let manifest: PatchManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|error| format!("invalid PATCH_MANIFEST.json: {error}"))?;
        validate_manifest(&manifest)?;

        let expected = manifest
            .files
            .iter()
            .map(|entry| format!("payload/{}", entry.path.replace('\\', "/")))
            .collect::<BTreeSet<_>>();
        let actual_payload = names
            .iter()
            .filter(|name| name.starts_with("payload/"))
            .cloned()
            .collect::<BTreeSet<_>>();
        if expected != actual_payload {
            return Err("Forge patch payload membership does not exactly match the manifest".to_owned());
        }
        let allowed = expected
            .iter()
            .cloned()
            .chain(std::iter::once("PATCH_MANIFEST.json".to_owned()))
            .collect::<BTreeSet<_>>();
        if names != allowed {
            return Err("Forge patch contains undeclared transport members".to_owned());
        }

        let mut total_payload_bytes = 0u64;
        for entry in &manifest.files {
            validate_relative_path(&entry.path)?;
            validate_sha256(&entry.sha256)?;
            let member = format!("payload/{}", entry.path.replace('\\', "/"));
            let bytes = read_member(&mut archive, &member)?;
            if bytes.len() as u64 != entry.bytes {
                return Err(format!("payload byte count mismatch: {}", entry.path));
            }
            if sha256_bytes(&bytes) != entry.sha256.to_ascii_lowercase() {
                return Err(format!("payload SHA-256 mismatch: {}", entry.path));
            }
            total_payload_bytes = total_payload_bytes.saturating_add(entry.bytes);
        }

        let validation = PatchValidation {
            patch_id: manifest.patch_id.clone(),
            project_id: manifest.target.project_id.clone(),
            transport_sha256,
            manifest_sha256: sha256_bytes(&manifest_bytes),
            file_count: manifest.files.len(),
            total_payload_bytes,
            base_commit: non_empty(&manifest.preconditions.git_commit),
        };
        Ok(Self { path, manifest, validation })
    }

    #[must_use]
    pub fn path(&self) -> &Path { &self.path }

    #[must_use]
    pub fn manifest(&self) -> &PatchManifest { &self.manifest }

    #[must_use]
    pub fn validation(&self) -> &PatchValidation { &self.validation }

    pub fn verify_project(&self, project_root: &Path) -> Result<(), String> {
        let contract_path = project_root.join("project.control.json");
        let contract: serde_json::Value = serde_json::from_slice(
            &fs::read(&contract_path)
                .map_err(|error| format!("failed to read {}: {error}", contract_path.display()))?,
        )
        .map_err(|error| format!("invalid project.control.json: {error}"))?;
        let project_id = contract
            .pointer("/project/id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if project_id != self.manifest.target.project_id {
            return Err(format!(
                "patch targets project `{}` but active project is `{project_id}`",
                self.manifest.target.project_id
            ));
        }
        if !self.manifest.requires.project_schema.trim().is_empty() {
            let schema = contract.get("schema").and_then(serde_json::Value::as_str).unwrap_or_default();
            if schema != self.manifest.requires.project_schema {
                return Err(format!(
                    "patch requires project schema `{}` but active contract is `{schema}`",
                    self.manifest.requires.project_schema
                ));
            }
        }
        if let Some(required) = non_empty(&self.manifest.preconditions.git_commit) {
            let head = git_head(project_root)?;
            if head != required {
                return Err(format!("patch base commit mismatch: required={required} actual={head}"));
            }
        }
        for entry in &self.manifest.files {
            let destination = project_root.join(path_from_slash(&entry.path));
            match entry.operation {
                PatchOperation::Add if destination.exists() => {
                    return Err(format!("patch add destination already exists: {}", entry.path));
                }
                PatchOperation::Replace if !destination.is_file() => {
                    return Err(format!("patch replace destination is missing: {}", entry.path));
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn apply_transactional(
        &self,
        project_root: &Path,
        state_root: &Path,
    ) -> Result<PatchReceipt, String> {
        self.verify_project(project_root)?;
        let transaction_root = state_root
            .join("patch-transactions")
            .join(format!("{}-{}", sanitize_id(&self.manifest.patch_id), unix_ms()));
        let stage_root = transaction_root.join("stage");
        let backup_root = transaction_root.join("backup");
        fs::create_dir_all(&stage_root).map_err(|error| error.to_string())?;
        fs::create_dir_all(&backup_root).map_err(|error| error.to_string())?;

        let file = File::open(&self.path).map_err(|error| error.to_string())?;
        let mut archive = ZipArchive::new(file).map_err(|error| error.to_string())?;
        for entry in &self.manifest.files {
            let member = format!("payload/{}", entry.path.replace('\\', "/"));
            let bytes = read_member(&mut archive, &member)?;
            let stage = stage_root.join(path_from_slash(&entry.path));
            write_new_file(&stage, &bytes)?;
        }

        let mut applied = Vec::<(PatchOperation, String)>::new();
        let result = (|| {
            for entry in &self.manifest.files {
                let rel = entry.path.clone();
                let destination = project_root.join(path_from_slash(&rel));
                let staged = stage_root.join(path_from_slash(&rel));
                if matches!(entry.operation, PatchOperation::Replace) {
                    let backup = backup_root.join(path_from_slash(&rel));
                    copy_verified(&destination, &backup)?;
                }
                publish_staged(&staged, &destination)?;
                applied.push((entry.operation.clone(), rel));
            }
            Ok::<(), String>(())
        })();

        if let Err(error) = result {
            let rollback_error = rollback(project_root, &backup_root, &applied).err();
            let message = rollback_error.map_or_else(
                || format!("apply failed and rollback completed: {error}"),
                |rollback| format!("apply failed: {error}; rollback also failed: {rollback}"),
            );
            let receipt = self.receipt(
                PatchReceiptStatus::Failed,
                transaction_root,
                applied.iter().map(|(_, path)| path.clone()).collect(),
                message.clone(),
            );
            let _ = write_receipt(state_root, &receipt);
            return Err(message);
        }

        let receipt = self.receipt(
            PatchReceiptStatus::Applied,
            transaction_root,
            applied.iter().map(|(_, path)| path.clone()).collect(),
            "patch applied transactionally; backups retained for recovery".to_owned(),
        );
        write_receipt(state_root, &receipt)?;
        Ok(receipt)
    }

    fn receipt(
        &self,
        status: PatchReceiptStatus,
        transaction_root: PathBuf,
        changed_paths: Vec<String>,
        message: String,
    ) -> PatchReceipt {
        PatchReceipt {
            schema: RECEIPT_SCHEMA.to_owned(),
            patch_id: self.manifest.patch_id.clone(),
            project_id: self.manifest.target.project_id.clone(),
            status,
            transport_sha256: self.validation.transport_sha256.clone(),
            transaction_root,
            changed_paths,
            message,
            unix_ms: unix_ms(),
        }
    }
}

fn validate_manifest(manifest: &PatchManifest) -> Result<(), String> {
    if manifest.schema != PATCH_SCHEMA {
        return Err(format!("unsupported Forge patch schema: {}", manifest.schema));
    }
    if manifest.engine != PATCH_ENGINE {
        return Err(format!("unsupported Forge patch engine: {}", manifest.engine));
    }
    if manifest.patch_id.trim().len() < 3 || !manifest.patch_id.chars().all(id_char) {
        return Err("invalid Forge patch id".to_owned());
    }
    if manifest.target.project_id.trim().is_empty() {
        return Err("Forge patch target.projectId is required".to_owned());
    }
    if manifest.files.is_empty() {
        return Err("Forge patch contains no files".to_owned());
    }
    let mut seen = BTreeSet::new();
    for entry in &manifest.files {
        validate_relative_path(&entry.path)?;
        if !seen.insert(entry.path.to_ascii_lowercase()) {
            return Err(format!("duplicate Forge patch path: {}", entry.path));
        }
    }
    Ok(())
}

fn regular_names(archive: &mut ZipArchive<File>) -> Result<BTreeSet<String>, String> {
    let mut names = BTreeSet::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|error| error.to_string())?;
        if entry.is_dir() { continue; }
        let name = entry.name().replace('\\', "/");
        validate_zip_name(&name)?;
        if !names.insert(name) {
            return Err("duplicate Forge patch ZIP member".to_owned());
        }
    }
    Ok(names)
}

fn read_member(archive: &mut ZipArchive<File>, name: &str) -> Result<Vec<u8>, String> {
    let mut entry = archive.by_name(name).map_err(|error| format!("missing ZIP member `{name}`: {error}"))?;
    let mut bytes = Vec::with_capacity(entry.size().min(64 * 1024 * 1024) as usize);
    entry.read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn validate_zip_name(value: &str) -> Result<(), String> {
    if value.starts_with('/') || value.contains("../") || value.contains("..\\") {
        return Err(format!("unsafe Forge patch ZIP path: {value}"));
    }
    validate_relative_path(value)
}

fn validate_relative_path(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.trim().is_empty() || path.is_absolute() {
        return Err(format!("unsafe Forge patch path: {value}"));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => return Err(format!("unsafe Forge patch path: {value}")),
        }
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid SHA-256 in Forge patch manifest".to_owned());
    }
    Ok(())
}

fn verify_sidecar_if_present(path: &Path, actual: &str) -> Result<(), String> {
    let sidecar = PathBuf::from(format!("{}.sha256", path.display()));
    if !sidecar.is_file() { return Ok(()); }
    let text = fs::read_to_string(&sidecar).map_err(|error| error.to_string())?;
    let expected = text.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    validate_sha256(&expected)?;
    if expected != actual {
        return Err(format!("Forge patch transport SHA-256 mismatch: {}", path.display()));
    }
    Ok(())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex_digest(&Sha256::digest(bytes))
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 { break; }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_digest(&hasher.finalize()))
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn git_head(root: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("failed to launch git: {error}"))?;
    if !output.status.success() {
        return Err("unable to resolve Git HEAD for Forge patch precondition".to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    if path.exists() {
        return Err(format!("staging path already exists: {}", path.display()));
    }
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    file.flush().map_err(|error| error.to_string())?;
    let _ = file.sync_all();
    Ok(())
}

fn copy_verified(source: &Path, destination: &Path) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::copy(source, destination).map_err(|error| error.to_string())?;
    let source_sha = sha256_file(source)?;
    let destination_sha = sha256_file(destination)?;
    if source_sha != destination_sha {
        return Err(format!("backup verification failed: {}", source.display()));
    }
    Ok(())
}

fn publish_staged(staged: &Path, destination: &Path) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temp = destination.with_extension(format!("forge-tmp-{}", unix_ms()));
    fs::copy(staged, &temp).map_err(|error| error.to_string())?;
    if destination.exists() {
        fs::remove_file(destination).map_err(|error| error.to_string())?;
    }
    fs::rename(&temp, destination).map_err(|error| error.to_string())
}

fn rollback(
    project_root: &Path,
    backup_root: &Path,
    applied: &[(PatchOperation, String)],
) -> Result<(), String> {
    for (operation, rel) in applied.iter().rev() {
        let destination = project_root.join(path_from_slash(rel));
        match operation {
            PatchOperation::Add => {
                if destination.exists() {
                    fs::remove_file(&destination).map_err(|error| error.to_string())?;
                }
            }
            PatchOperation::Replace => {
                let backup = backup_root.join(path_from_slash(rel));
                if destination.exists() {
                    fs::remove_file(&destination).map_err(|error| error.to_string())?;
                }
                copy_verified(&backup, &destination)?;
            }
        }
    }
    Ok(())
}

fn write_receipt(state_root: &Path, receipt: &PatchReceipt) -> Result<(), String> {
    let root = state_root.join("patch-receipts");
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let path = root.join(format!("{}-{}.json", sanitize_id(&receipt.patch_id), receipt.unix_ms));
    let bytes = serde_json::to_vec_pretty(receipt).map_err(|error| error.to_string())?;
    let mut file = File::create(&path).map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.flush().map_err(|error| error.to_string())
}

fn path_from_slash(value: &str) -> PathBuf {
    value.split('/').filter(|part| !part.is_empty()).collect()
}

fn sanitize_id(value: &str) -> String {
    value.chars().map(|character| if id_char(character) { character } else { '_' }).collect()
}

fn id_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() { None } else { Some(value.to_owned()) }
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_parent_paths() {
        assert!(validate_relative_path("../bad.txt").is_err());
        assert!(validate_relative_path("good/path.txt").is_ok());
    }

    #[test]
    fn manifest_requires_modern_schema_and_unique_paths() {
        let mut manifest = PatchManifest::default();
        manifest.schema = PATCH_SCHEMA.to_owned();
        manifest.engine = PATCH_ENGINE.to_owned();
        manifest.patch_id = "TEST-PATCH-01".to_owned();
        manifest.target.project_id = "demo".to_owned();
        manifest.files = vec![PatchFile {
            path: "src/lib.rs".to_owned(),
            operation: PatchOperation::Add,
            bytes: 1,
            sha256: "0".repeat(64),
        }];
        assert!(validate_manifest(&manifest).is_ok());
        manifest.files.push(manifest.files[0].clone());
        assert!(validate_manifest(&manifest).is_err());
    }
}
