use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const FORGE_STATE_SCHEMA: &str = "forge.state.v1";
pub const FORGE_STATE_SCHEMA_VERSION: u32 = 1;

static ID_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ForgeSettings {
    pub schema_version: u32,
    pub scan_roots: Vec<PathBuf>,
    pub max_scan_depth: usize,
    pub max_scan_directories: usize,
    pub artifact_central_root: Option<PathBuf>,
    pub internal_git_root: Option<PathBuf>,
    pub intake_roots: Vec<PathBuf>,
    pub downloads_auto_queue: bool,
    pub patch_stale_age_hours: u64,
    pub intake_poll_seconds: u64,
    pub max_parallel_jobs: usize,
    pub classified_transport_auto_archive: bool,
}

impl Default for ForgeSettings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            scan_roots: Vec::new(),
            max_scan_depth: 12,
            max_scan_directories: 250_000,
            artifact_central_root: existing_path_from_env_or_default(
                "FORGE_ARTIFACT_CENTRAL_ROOT",
                r"D:\Vault\ArtifactCentral",
            ),
            internal_git_root: existing_path_from_env_or_default(
                "FORGE_INTERNAL_GIT_ROOT",
                r"D:\Forge\InternalGit",
            ),
            intake_roots: default_intake_roots(),
            downloads_auto_queue: false,
            patch_stale_age_hours: 1,
            intake_poll_seconds: 5,
            max_parallel_jobs: 1,
            classified_transport_auto_archive: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub root: PathBuf,
    pub contract_schema: String,
    pub contract_schema_version: u32,
    pub parent_id: Option<String>,
    pub child_ids: Vec<String>,
    pub duplicate_roots: Vec<PathBuf>,
    pub last_seen_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveryReport {
    pub scanned_roots: Vec<PathBuf>,
    pub scanned_directories: usize,
    pub project_count: usize,
    pub duplicate_project_ids: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactStatus {
    Discovered,
    Verified,
    Promoted,
    Archived,
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactRecord {
    pub id: String,
    pub project_id: String,
    pub kind: String,
    pub path: PathBuf,
    pub status: ArtifactStatus,
    pub sha256: Option<String>,
    pub operation_id: Option<String>,
    pub created_unix_ms: u64,
    pub updated_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PatchDisposition {
    Pending,
    Queued,
    Applied,
    Superseded,
    Stale,
    Incompatible,
    Invalid,
    Archived,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchEvidence {
    pub patch_id: String,
    pub project_id: String,
    pub source_path: PathBuf,
    pub created_unix_ms: Option<u64>,
    pub base_commit: Option<String>,
    pub applied: bool,
    pub superseded: bool,
    pub valid_manifest: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchLineageRecord {
    pub patch_id: String,
    pub project_id: String,
    pub source_path: PathBuf,
    pub disposition: PatchDisposition,
    pub reason: String,
    pub base_commit: Option<String>,
    pub created_unix_ms: Option<u64>,
    pub observed_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    Unknown,
    Stopped,
    Starting,
    Running,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceRecord {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub state: ServiceState,
    pub endpoint: Option<String>,
    pub pid: Option<u32>,
    pub detail: String,
    pub updated_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueueState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueueRecord {
    pub id: String,
    pub project_id: String,
    pub operation: String,
    pub state: QueueState,
    pub created_unix_ms: u64,
    pub updated_unix_ms: u64,
    pub lease_id: Option<String>,
    pub operation_id: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceControlLaneKind {
    Github,
    InternalGit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceControlLane {
    pub kind: SourceControlLaneKind,
    pub configured: bool,
    pub authority: String,
    pub location: Option<String>,
    pub branch: Option<String>,
    pub clean: Option<bool>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    pub last_sync_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceControlRecord {
    pub project_id: String,
    pub github: SourceControlLane,
    pub internal_git: SourceControlLane,
    pub updated_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostWorkspaceKind {
    Cortex,
    Ember,
    Ide,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostWorkspaceState {
    Unavailable,
    Candidate,
    Ready,
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostWorkspaceRecord {
    pub kind: HostWorkspaceKind,
    pub state: HostWorkspaceState,
    pub detail: String,
    pub updated_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TakeoverStatus {
    Missing,
    Candidate,
    Verified,
    Blocked,
    NotApplicable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TakeoverCheck {
    pub id: String,
    pub label: String,
    pub status: TakeoverStatus,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TakeoverMatrix {
    pub schema_version: u32,
    pub checks: Vec<TakeoverCheck>,
}

impl Default for TakeoverMatrix {
    fn default() -> Self {
        Self {
            schema_version: 1,
            checks: vec![
                takeover("project_contract", "forge.project.v1 contract", TakeoverStatus::Verified),
                takeover("durable_operations", "Durable operation receipts/logs/cancellation", TakeoverStatus::Verified),
                takeover("project_registry", "Fleet and nested-project registry", TakeoverStatus::Candidate),
                takeover("patch_engine", "Native forge.patch.v1 validation/apply/rollback", TakeoverStatus::Candidate),
                takeover("artifact_central", "Artifact Central authority", TakeoverStatus::Candidate),
                takeover("patch_lineage", "Patch intake and lineage", TakeoverStatus::Candidate),
                takeover("intake_watcher", "Downloads/intake classification watcher", TakeoverStatus::Candidate),
                takeover("service_manager", "Service lifecycle and health", TakeoverStatus::Candidate),
                takeover("multi_project_queue", "Durable multi-project scheduler execution", TakeoverStatus::Candidate),
                takeover("github", "GitHub source-control lane", TakeoverStatus::Candidate),
                takeover("internal_git", "Forge Repository/Internal Git lane", TakeoverStatus::Candidate),
                takeover("cortex_host", "Forge-hosted Cortex workspace", TakeoverStatus::Candidate),
                takeover("ide", "Native Rust Forge IDE workspace (no WebView)", TakeoverStatus::Candidate),
                takeover("ide_lsp_dap", "Native IDE LSP/DAP/terminal adapter contracts", TakeoverStatus::Candidate),
                takeover("filesystem_watch", "Persistent filesystem/intake watch snapshots", TakeoverStatus::Candidate),
                takeover("toolchain_doctor", "Project toolchain/dependency doctor", TakeoverStatus::Candidate),
                takeover("native_diagnostics", "Native Forge diagnostics aggregation", TakeoverStatus::Candidate),
                takeover("ember_host", "Forge-hosted Ember adapter contract", TakeoverStatus::Candidate),
                takeover("platform", "Platform notification/tray state", TakeoverStatus::Candidate),
                takeover("system_tray", "Native system tray and Windows notifications", TakeoverStatus::Missing),
                takeover("release", "Portable release manifest and recovery checkpoints", TakeoverStatus::Candidate),
                takeover("installer", "Installer/updater/recovery", TakeoverStatus::Missing),
                takeover("secure_credentials", "OS-backed secure credential storage", TakeoverStatus::Missing),
                takeover("takeover_harness", "ForgePY/Rust Forge parity certification", TakeoverStatus::Candidate),
            ],
        }
    }
}

impl TakeoverMatrix {
    #[must_use]
    pub fn ready_for_takeover(&self) -> bool {
        self.checks.iter().all(|check| {
            matches!(check.status, TakeoverStatus::Verified | TakeoverStatus::NotApplicable)
        })
    }

    pub fn set(&mut self, id: &str, status: TakeoverStatus, evidence: impl Into<String>) {
        if let Some(check) = self.checks.iter_mut().find(|check| check.id == id) {
            check.status = status;
            check.evidence = evidence.into();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForgeStateSnapshot {
    pub schema: String,
    pub schema_version: u32,
    pub settings: ForgeSettings,
    pub active_project_id: Option<String>,
    pub projects: Vec<ProjectRecord>,
    pub artifacts: Vec<ArtifactRecord>,
    pub patch_lineage: Vec<PatchLineageRecord>,
    pub services: Vec<ServiceRecord>,
    pub queue: VecDeque<QueueRecord>,
    pub source_control: Vec<SourceControlRecord>,
    pub host_workspaces: Vec<HostWorkspaceRecord>,
    pub takeover: TakeoverMatrix,
    pub updated_unix_ms: u64,
}

impl Default for ForgeStateSnapshot {
    fn default() -> Self {
        Self {
            schema: FORGE_STATE_SCHEMA.to_owned(),
            schema_version: FORGE_STATE_SCHEMA_VERSION,
            settings: ForgeSettings::default(),
            active_project_id: None,
            projects: Vec::new(),
            artifacts: Vec::new(),
            patch_lineage: Vec::new(),
            services: Vec::new(),
            queue: VecDeque::new(),
            source_control: Vec::new(),
            host_workspaces: Vec::new(),
            takeover: TakeoverMatrix::default(),
            updated_unix_ms: unix_ms(),
        }
    }
}

pub struct ForgeStateStore {
    root: PathBuf,
    snapshot: ForgeStateSnapshot,
}

impl ForgeStateStore {
    pub fn open_default(active_project_root: &Path) -> Result<Self, String> {
        let root = default_state_root(active_project_root);
        Self::open(root)
    }

    pub fn open(root: impl AsRef<Path>) -> Result<Self, String> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)
            .map_err(|error| format!("failed to create Forge state root {}: {error}", root.display()))?;
        let path = root.join("forge-state.json");
        let snapshot = if path.is_file() {
            let bytes = fs::read(&path)
                .map_err(|error| format!("failed to read Forge state {}: {error}", path.display()))?;
            let snapshot: ForgeStateSnapshot = serde_json::from_slice(&bytes)
                .map_err(|error| format!("failed to parse Forge state {}: {error}", path.display()))?;
            validate_snapshot(snapshot)?
        } else {
            ForgeStateSnapshot::default()
        };
        Ok(Self { root, snapshot })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn snapshot(&self) -> &ForgeStateSnapshot {
        &self.snapshot
    }

    pub fn save(&mut self) -> Result<(), String> {
        self.snapshot.updated_unix_ms = unix_ms();
        let bytes = serde_json::to_vec_pretty(&self.snapshot)
            .map_err(|error| format!("failed to serialize Forge state: {error}"))?;
        atomic_write(&self.root.join("forge-state.json"), &bytes)
    }

    pub fn register_project_root(&mut self, root: &Path) -> Result<ProjectRecord, String> {
        let record = read_project_record(root)?;
        merge_project(&mut self.snapshot.projects, record.clone());
        self.snapshot.active_project_id = Some(record.id.clone());
        if !self.snapshot.settings.scan_roots.iter().any(|item| same_path(item, root)) {
            if let Some(parent) = root.parent() {
                self.snapshot.settings.scan_roots.push(parent.to_path_buf());
                dedupe_paths(&mut self.snapshot.settings.scan_roots);
            }
        }
        assign_project_graph(&mut self.snapshot.projects);
        self.save()?;
        Ok(record)
    }

    pub fn set_active_project(&mut self, project_id: &str) -> Result<(), String> {
        if !self.snapshot.projects.iter().any(|project| project.id == project_id) {
            return Err(format!("project is not registered in Forge state: {project_id}"));
        }
        self.snapshot.active_project_id = Some(project_id.to_owned());
        self.save()
    }

    pub fn update_settings(&mut self, update: impl FnOnce(&mut ForgeSettings)) -> Result<(), String> {
        update(&mut self.snapshot.settings);
        self.snapshot.settings.max_scan_depth = self.snapshot.settings.max_scan_depth.clamp(1, 64);
        self.snapshot.settings.max_scan_directories = self.snapshot.settings.max_scan_directories.max(1_000);
        self.snapshot.settings.intake_poll_seconds = self.snapshot.settings.intake_poll_seconds.clamp(1, 3_600);
        self.snapshot.settings.max_parallel_jobs = self.snapshot.settings.max_parallel_jobs.clamp(1, 32);
        self.save()
    }

    #[must_use]
    pub fn project(&self, project_id: &str) -> Option<&ProjectRecord> {
        self.snapshot.projects.iter().find(|project| project.id == project_id)
    }

    pub fn add_scan_root(&mut self, root: impl AsRef<Path>) -> Result<(), String> {
        let root = root.as_ref().to_path_buf();
        if !root.is_dir() {
            return Err(format!("Forge scan root is not a directory: {}", root.display()));
        }
        self.snapshot.settings.scan_roots.push(root);
        dedupe_paths(&mut self.snapshot.settings.scan_roots);
        self.save()
    }

    pub fn scan_projects(&mut self) -> Result<DiscoveryReport, String> {
        let roots = self.snapshot.settings.scan_roots.clone();
        let mut state = ScanState::default();
        for root in &roots {
            scan_root(
                root,
                0,
                self.snapshot.settings.max_scan_depth,
                self.snapshot.settings.max_scan_directories,
                &mut state,
            )?;
            if state.truncated {
                break;
            }
        }

        let mut by_id: BTreeMap<String, ProjectRecord> = BTreeMap::new();
        for record in state.projects {
            match by_id.get_mut(&record.id) {
                Some(existing) => {
                    if !same_path(&existing.root, &record.root)
                        && !existing.duplicate_roots.iter().any(|root| same_path(root, &record.root))
                    {
                        existing.duplicate_roots.push(record.root);
                    }
                }
                None => {
                    by_id.insert(record.id.clone(), record);
                }
            }
        }
        let mut projects = by_id.into_values().collect::<Vec<_>>();
        assign_project_graph(&mut projects);
        let duplicate_project_ids = projects
            .iter()
            .filter(|project| !project.duplicate_roots.is_empty())
            .count();
        let project_count = projects.len();
        self.snapshot.projects = projects;
        if let Some(active) = self.snapshot.active_project_id.clone() {
            if !self.snapshot.projects.iter().any(|project| project.id == active) {
                self.snapshot.active_project_id = None;
            }
        }
        self.save()?;
        Ok(DiscoveryReport {
            scanned_roots: roots,
            scanned_directories: state.directories,
            project_count,
            duplicate_project_ids,
            truncated: state.truncated,
        })
    }

    pub fn upsert_artifact(&mut self, mut record: ArtifactRecord) -> Result<(), String> {
        record.updated_unix_ms = unix_ms();
        if record.created_unix_ms == 0 {
            record.created_unix_ms = record.updated_unix_ms;
        }
        if let Some(existing) = self.snapshot.artifacts.iter_mut().find(|item| item.id == record.id) {
            *existing = record;
        } else {
            self.snapshot.artifacts.push(record);
        }
        self.save()
    }

    pub fn classify_patch(
        &mut self,
        evidence: PatchEvidence,
        current_commit: Option<&str>,
    ) -> Result<PatchLineageRecord, String> {
        let now = unix_ms();
        let stale_ms = self
            .snapshot
            .settings
            .patch_stale_age_hours
            .saturating_mul(60)
            .saturating_mul(60)
            .saturating_mul(1_000);
        let active_project = self.snapshot.active_project_id.as_deref();
        let (disposition, reason) = if !evidence.valid_manifest {
            (PatchDisposition::Invalid, "manifest validation failed".to_owned())
        } else if evidence.applied {
            (PatchDisposition::Applied, "patch lineage already records this update as applied".to_owned())
        } else if evidence.superseded {
            (PatchDisposition::Superseded, "a newer patch in the same lineage supersedes this update".to_owned())
        } else if active_project.is_some_and(|active| active != evidence.project_id) {
            (PatchDisposition::Incompatible, "patch targets a different active project".to_owned())
        } else if evidence
            .base_commit
            .as_deref()
            .zip(current_commit)
            .is_some_and(|(base, current)| base != current)
        {
            (PatchDisposition::Incompatible, "patch base commit does not match current source authority".to_owned())
        } else if evidence
            .created_unix_ms
            .is_some_and(|created| stale_ms > 0 && now.saturating_sub(created) > stale_ms)
        {
            (PatchDisposition::Stale, format!("patch is older than the configured {} hour intake window", self.snapshot.settings.patch_stale_age_hours))
        } else {
            (PatchDisposition::Pending, "patch passed lineage preclassification and still requires explicit approval".to_owned())
        };
        let record = PatchLineageRecord {
            patch_id: evidence.patch_id,
            project_id: evidence.project_id,
            source_path: evidence.source_path,
            disposition,
            reason,
            base_commit: evidence.base_commit,
            created_unix_ms: evidence.created_unix_ms,
            observed_unix_ms: now,
        };
        if let Some(existing) = self
            .snapshot
            .patch_lineage
            .iter_mut()
            .find(|item| item.patch_id == record.patch_id && item.project_id == record.project_id)
        {
            *existing = record.clone();
        } else {
            self.snapshot.patch_lineage.push(record.clone());
        }
        self.save()?;
        Ok(record)
    }

    pub fn upsert_service(&mut self, mut service: ServiceRecord) -> Result<(), String> {
        service.updated_unix_ms = unix_ms();
        if let Some(existing) = self.snapshot.services.iter_mut().find(|item| item.id == service.id) {
            *existing = service;
        } else {
            self.snapshot.services.push(service);
        }
        self.save()
    }

    pub fn enqueue(&mut self, project_id: &str, operation: &str) -> Result<QueueRecord, String> {
        if !self.snapshot.projects.iter().any(|project| project.id == project_id) {
            return Err(format!("cannot enqueue operation for unknown project: {project_id}"));
        }
        let now = unix_ms();
        let record = QueueRecord {
            id: new_id("forgejob"),
            project_id: project_id.to_owned(),
            operation: operation.to_owned(),
            state: QueueState::Queued,
            created_unix_ms: now,
            updated_unix_ms: now,
            lease_id: None,
            operation_id: None,
            detail: String::new(),
        };
        self.snapshot.queue.push_back(record.clone());
        self.save()?;
        Ok(record)
    }

    pub fn claim_next(&mut self) -> Result<Option<QueueRecord>, String> {
        let Some(record) = self
            .snapshot
            .queue
            .iter_mut()
            .find(|record| record.state == QueueState::Queued)
        else {
            return Ok(None);
        };
        record.state = QueueState::Running;
        record.updated_unix_ms = unix_ms();
        record.lease_id = Some(new_id("lease"));
        let result = record.clone();
        self.save()?;
        Ok(Some(result))
    }

    pub fn finish_queue_item(
        &mut self,
        id: &str,
        state: QueueState,
        operation_id: Option<String>,
        detail: impl Into<String>,
    ) -> Result<(), String> {
        if matches!(state, QueueState::Queued | QueueState::Running) {
            return Err("queue completion requires a terminal state".to_owned());
        }
        let record = self
            .snapshot
            .queue
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or_else(|| format!("Forge queue item not found: {id}"))?;
        record.state = state;
        record.updated_unix_ms = unix_ms();
        record.operation_id = operation_id;
        record.detail = detail.into();
        record.lease_id = None;
        self.save()
    }

    pub fn recover_running_queue(&mut self) -> Result<usize, String> {
        let now = unix_ms();
        let mut count = 0usize;
        for record in &mut self.snapshot.queue {
            if record.state == QueueState::Running {
                record.state = QueueState::Interrupted;
                record.updated_unix_ms = now;
                record.lease_id = None;
                record.detail = "Forge restarted before the queue item reached a terminal state".to_owned();
                count += 1;
            }
        }
        if count > 0 {
            self.save()?;
        }
        Ok(count)
    }

    pub fn upsert_source_control(&mut self, mut record: SourceControlRecord) -> Result<(), String> {
        record.updated_unix_ms = unix_ms();
        if let Some(existing) = self
            .snapshot
            .source_control
            .iter_mut()
            .find(|item| item.project_id == record.project_id)
        {
            *existing = record;
        } else {
            self.snapshot.source_control.push(record);
        }
        self.save()
    }

    pub fn upsert_host_workspace(&mut self, mut record: HostWorkspaceRecord) -> Result<(), String> {
        record.updated_unix_ms = unix_ms();
        if let Some(existing) = self
            .snapshot
            .host_workspaces
            .iter_mut()
            .find(|item| item.kind == record.kind)
        {
            *existing = record;
        } else {
            self.snapshot.host_workspaces.push(record);
        }
        self.save()
    }

    pub fn set_takeover_check(
        &mut self,
        id: &str,
        status: TakeoverStatus,
        evidence: impl Into<String>,
    ) -> Result<(), String> {
        self.snapshot.takeover.set(id, status, evidence);
        self.save()
    }

    pub fn write_diagnostics_snapshot(&mut self) -> Result<PathBuf, String> {
        self.snapshot.updated_unix_ms = unix_ms();
        let path = self.root.join("diagnostics.json");
        let bytes = serde_json::to_vec_pretty(&self.snapshot)
            .map_err(|error| format!("failed to serialize Forge diagnostics: {error}"))?;
        atomic_write(&path, &bytes)?;
        Ok(path)
    }
}

#[derive(Default)]
struct ScanState {
    directories: usize,
    projects: Vec<ProjectRecord>,
    visited: BTreeSet<PathBuf>,
    truncated: bool,
}

fn scan_root(
    root: &Path,
    depth: usize,
    max_depth: usize,
    max_directories: usize,
    state: &mut ScanState,
) -> Result<(), String> {
    if state.truncated || depth > max_depth || !root.is_dir() {
        return Ok(());
    }
    let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    if !state.visited.insert(canonical) {
        return Ok(());
    }
    state.directories = state.directories.saturating_add(1);
    if state.directories > max_directories {
        state.truncated = true;
        return Ok(());
    }

    if root.join("project.control.json").is_file() {
        if let Ok(record) = read_project_record(root) {
            state.projects.push(record);
        }
    }

    if depth == max_depth {
        return Ok(());
    }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if ignored_directory(&name) {
            continue;
        }
        scan_root(
            &entry.path(),
            depth + 1,
            max_depth,
            max_directories,
            state,
        )?;
        if state.truncated {
            break;
        }
    }
    Ok(())
}

fn read_project_record(root: &Path) -> Result<ProjectRecord, String> {
    let path = root.join("project.control.json");
    let bytes = fs::read(&path)
        .map_err(|error| format!("failed to read project contract {}: {error}", path.display()))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("failed to parse project contract {}: {error}", path.display()))?;
    let project = value
        .get("project")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("project contract omitted project object: {}", path.display()))?;
    let id = required_string(project.get("id"), "project.id", &path)?;
    let name = required_string(project.get("name"), "project.name", &path)?;
    let kind = required_string(project.get("kind"), "project.kind", &path)?;
    let contract_schema = value
        .get("schema")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let contract_schema_version = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(u64::from(u32::MAX)) as u32;
    Ok(ProjectRecord {
        id,
        name,
        kind,
        root: fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()),
        contract_schema,
        contract_schema_version,
        parent_id: None,
        child_ids: Vec::new(),
        duplicate_roots: Vec::new(),
        last_seen_unix_ms: unix_ms(),
    })
}

fn required_string(value: Option<&Value>, field: &str, path: &Path) -> Result<String, String> {
    let value = value.and_then(Value::as_str).unwrap_or("").trim();
    if value.is_empty() {
        Err(format!("project contract field {field} is required: {}", path.display()))
    } else {
        Ok(value.to_owned())
    }
}

fn merge_project(projects: &mut Vec<ProjectRecord>, record: ProjectRecord) {
    if let Some(existing) = projects.iter_mut().find(|project| project.id == record.id) {
        if same_path(&existing.root, &record.root) {
            *existing = record;
        } else if !existing.duplicate_roots.iter().any(|root| same_path(root, &record.root)) {
            existing.duplicate_roots.push(record.root);
            existing.last_seen_unix_ms = unix_ms();
        }
    } else {
        projects.push(record);
    }
}

fn assign_project_graph(projects: &mut [ProjectRecord]) {
    for project in projects.iter_mut() {
        project.parent_id = None;
        project.child_ids.clear();
    }
    let identities = projects
        .iter()
        .map(|project| (project.id.clone(), project.root.clone()))
        .collect::<Vec<_>>();
    let mut parents = BTreeMap::new();
    for (id, root) in &identities {
        let parent = identities
            .iter()
            .filter(|(candidate_id, candidate_root)| {
                candidate_id != id && root.starts_with(candidate_root)
            })
            .max_by_key(|(_, candidate_root)| candidate_root.components().count())
            .map(|(candidate_id, _)| candidate_id.clone());
        if let Some(parent) = parent {
            parents.insert(id.clone(), parent);
        }
    }
    for project in projects.iter_mut() {
        project.parent_id = parents.get(&project.id).cloned();
    }
    let edges = projects
        .iter()
        .filter_map(|project| {
            project
                .parent_id
                .as_ref()
                .map(|parent| (parent.clone(), project.id.clone()))
        })
        .collect::<Vec<_>>();
    for (parent, child) in edges {
        if let Some(record) = projects.iter_mut().find(|project| project.id == parent) {
            record.child_ids.push(child);
            record.child_ids.sort();
            record.child_ids.dedup();
        }
    }
}

fn ignored_directory(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        ".git"
            | ".hg"
            | ".svn"
            | "target"
            | "node_modules"
            | ".venv"
            | "venv"
            | "artifacts"
            | "logs"
            | "dist"
            | "build"
            | ".project_control"
            | ".cortex"
    )
}

fn validate_snapshot(mut snapshot: ForgeStateSnapshot) -> Result<ForgeStateSnapshot, String> {
    if snapshot.schema != FORGE_STATE_SCHEMA {
        return Err(format!("unsupported Forge state schema: {}", snapshot.schema));
    }
    if snapshot.schema_version != FORGE_STATE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported Forge state schema version: {}",
            snapshot.schema_version
        ));
    }
    assign_project_graph(&mut snapshot.projects);
    snapshot.settings.max_scan_depth = snapshot.settings.max_scan_depth.clamp(1, 64);
    snapshot.settings.max_scan_directories = snapshot.settings.max_scan_directories.max(1_000);
    snapshot.settings.intake_poll_seconds = snapshot.settings.intake_poll_seconds.clamp(1, 3_600);
    snapshot.settings.max_parallel_jobs = snapshot.settings.max_parallel_jobs.clamp(1, 32);
    for default_check in TakeoverMatrix::default().checks {
        if !snapshot.takeover.checks.iter().any(|check| check.id == default_check.id) {
            snapshot.takeover.checks.push(default_check);
        }
    }
    Ok(snapshot)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    let temp = parent.join(format!(
        ".forge-state-{}-{}.tmp",
        std::process::id(),
        ID_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    {
        let mut file = File::create(&temp)
            .map_err(|error| format!("failed to create {}: {error}", temp.display()))?;
        file.write_all(bytes)
            .map_err(|error| format!("failed to write {}: {error}", temp.display()))?;
        file.write_all(b"\n")
            .map_err(|error| format!("failed to finalize {}: {error}", temp.display()))?;
        file.flush()
            .map_err(|error| format!("failed to flush {}: {error}", temp.display()))?;
        let _ = file.sync_all();
    }
    if path.exists() {
        fs::remove_file(path)
            .map_err(|error| format!("failed to replace {}: {error}", path.display()))?;
    }
    fs::rename(&temp, path).map_err(|error| {
        format!(
            "failed to publish Forge state {} -> {}: {error}",
            temp.display(),
            path.display()
        )
    })
}

fn default_state_root(active_project_root: &Path) -> PathBuf {
    if let Some(root) = env::var_os("FORGE_STATE_ROOT").filter(|value| !value.is_empty()) {
        return PathBuf::from(root);
    }
    if let Some(root) = env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
        return PathBuf::from(root).join("Forge").join("state");
    }
    if let Some(root) = env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(root).join("forge");
    }
    if let Some(home) = env::var_os("HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(home).join(".local").join("state").join("forge");
    }
    active_project_root
        .join("artifacts")
        .join("forge-rust")
        .join("machine-state")
}

fn default_intake_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(profile) = env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
        let downloads = PathBuf::from(profile).join("Downloads");
        if downloads.is_dir() {
            roots.push(downloads);
        }
    }
    roots
}

fn existing_path_from_env_or_default(key: &str, fallback: &str) -> Option<PathBuf> {
    if let Some(value) = env::var_os(key).filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(value));
    }
    let path = PathBuf::from(fallback);
    path.exists().then_some(path)
}

fn dedupe_paths(paths: &mut Vec<PathBuf>) {
    let mut seen = BTreeSet::new();
    paths.retain(|path| seen.insert(path_identity(path)));
}

fn same_path(left: &Path, right: &Path) -> bool {
    path_identity(left) == path_identity(right)
}

fn path_identity(path: &Path) -> String {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(windows)]
    {
        path.to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_owned()
    }
}

fn takeover(id: &str, label: &str, status: TakeoverStatus) -> TakeoverCheck {
    TakeoverCheck {
        id: id.to_owned(),
        label: label.to_owned(),
        status,
        evidence: String::new(),
    }
}

fn new_id(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}-{}",
        unix_ms(),
        std::process::id(),
        ID_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
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

    fn temp_root(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!(
            "forge-state-{name}-{}-{}",
            std::process::id(),
            ID_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("temp root");
        path
    }

    fn write_contract(root: &Path, id: &str, name: &str) {
        fs::create_dir_all(root).expect("project root");
        let value = serde_json::json!({
            "schema": "forge.project.v1",
            "schema_version": 1,
            "project": {"id": id, "name": name, "kind": "test"}
        });
        fs::write(
            root.join("project.control.json"),
            serde_json::to_vec_pretty(&value).expect("json"),
        )
        .expect("contract");
    }

    #[test]
    fn registry_preserves_nested_project_graph() {
        let base = temp_root("nested");
        let parent = base.join("parent");
        let child = parent.join("tools").join("child");
        write_contract(&parent, "parent", "Parent");
        write_contract(&child, "child", "Child");
        let state_root = base.join("state");
        let mut store = ForgeStateStore::open(&state_root).expect("store");
        store.snapshot.settings.scan_roots = vec![base.clone()];
        store.snapshot.settings.max_scan_depth = 8;
        let report = store.scan_projects().expect("scan");
        assert_eq!(report.project_count, 2);
        let parent_record = store
            .snapshot()
            .projects
            .iter()
            .find(|record| record.id == "parent")
            .expect("parent");
        assert_eq!(parent_record.child_ids, vec!["child".to_owned()]);
        let child_record = store
            .snapshot()
            .projects
            .iter()
            .find(|record| record.id == "child")
            .expect("child");
        assert_eq!(child_record.parent_id.as_deref(), Some("parent"));
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn queue_recovers_running_work_as_interrupted() {
        let base = temp_root("queue");
        let project = base.join("project");
        write_contract(&project, "project", "Project");
        let mut store = ForgeStateStore::open(base.join("state")).expect("store");
        store.register_project_root(&project).expect("register");
        let queued = store.enqueue("project", "gate.full").expect("enqueue");
        let claimed = store.claim_next().expect("claim").expect("record");
        assert_eq!(claimed.id, queued.id);
        assert_eq!(claimed.state, QueueState::Running);
        assert_eq!(store.recover_running_queue().expect("recover"), 1);
        assert_eq!(store.snapshot().queue[0].state, QueueState::Interrupted);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn stale_patch_is_quarantined_by_classification() {
        let base = temp_root("patch");
        let project = base.join("project");
        write_contract(&project, "project", "Project");
        let mut store = ForgeStateStore::open(base.join("state")).expect("store");
        store.register_project_root(&project).expect("register");
        store.snapshot.settings.patch_stale_age_hours = 1;
        let evidence = PatchEvidence {
            patch_id: "TEST-PATCH".to_owned(),
            project_id: "project".to_owned(),
            source_path: base.join("TEST-PATCH.patch"),
            created_unix_ms: Some(unix_ms().saturating_sub(2 * 60 * 60 * 1_000)),
            base_commit: None,
            applied: false,
            superseded: false,
            valid_manifest: true,
        };
        let record = store.classify_patch(evidence, None).expect("classify");
        assert_eq!(record.disposition, PatchDisposition::Stale);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn takeover_requires_every_required_surface_verified() {
        let mut matrix = TakeoverMatrix::default();
        assert!(!matrix.ready_for_takeover());
        for check in &mut matrix.checks {
            check.status = TakeoverStatus::Verified;
        }
        assert!(matrix.ready_for_takeover());
    }

    #[test]
    fn state_round_trip_is_schema_guarded() {
        let base = temp_root("roundtrip");
        let mut store = ForgeStateStore::open(&base).expect("store");
        store.save().expect("save");
        let reopened = ForgeStateStore::open(&base).expect("reopen");
        assert_eq!(reopened.snapshot().schema, FORGE_STATE_SCHEMA);
        assert_eq!(reopened.snapshot().schema_version, FORGE_STATE_SCHEMA_VERSION);
        let _ = fs::remove_dir_all(base);
    }
}
